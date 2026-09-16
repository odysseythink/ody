//! Read-only heuristic discovery for workspace project roots.
//!
//! The scanner never follows symlinks, never writes, and only reads small
//! manifest files (`package.json`, capped at 1 MiB). Everything else is
//! path-convention based; there is intentionally no AST parsing in E0.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ody_app_server_protocol::WorkspaceGitStatus;
use ody_app_server_protocol::WorkspaceRootDiscovery;
use ody_app_server_protocol::WorkspaceScanStats;
use ody_app_server_protocol::WorkspaceScript;
use ody_app_server_protocol::WorkspaceSourceEntry;
use ody_app_server_protocol::WorkspaceSourceKind;
use ody_app_server_protocol::WorkspaceTechEntry;
use ody_app_server_protocol::WorkspaceTreeEntry;
use ody_app_server_protocol::WorkspaceTreeEntryKind;

use crate::error_code::invalid_params;
use ody_app_server_protocol::JSONRPCErrorError;

/// Hard caps keep scans bounded on large trees. Exceeding a cap sets
/// `truncated` on the affected root instead of failing the scan.
pub(crate) const MAX_FILES_VISITED: u64 = 20_000;
pub(crate) const MAX_DEPTH: u32 = 12;
pub(crate) const MAX_PACKAGE_JSON_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_SOURCES_PER_KIND: usize = 500;

pub(crate) const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    "coverage",
    "vendor",
    "target",
    ".cache",
    ".output",
    "__pycache__",
];

pub(crate) const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "vue", "svelte"];

/// Hard caps for the on-demand tree listing (`workspace/project/tree`).
/// Exceeding a cap sets `truncated` instead of failing the listing.
pub(crate) const MAX_TREE_ENTRIES: usize = 2_000;
pub(crate) const MAX_TREE_REQUEST_DEPTH: u32 = 4;

pub(crate) struct TreeListing {
    pub entries: Vec<WorkspaceTreeEntry>,
    pub truncated: bool,
}

/// List `dir` (an absolute path inside a bound root, already normalized and
/// escape-checked by the caller) down to `depth` levels. Shares the scan's
/// skip-dir policy and never follows symlinks, so a listed directory can
/// never escape the root. Entries are sorted by name at every level.
pub(crate) fn read_tree(
    dir: &Path,
    relative: &str,
    depth: u32,
) -> Result<TreeListing, JSONRPCErrorError> {
    let mut listing = TreeListing {
        entries: Vec::new(),
        truncated: false,
    };
    if !dir.is_dir() {
        return Err(invalid_params(format!(
            "tree path {relative:?} is not a directory under the workspace root"
        )));
    }
    fill_tree_entries(dir, relative, depth.clamp(1, MAX_TREE_REQUEST_DEPTH), &mut listing);
    Ok(listing)
}

fn fill_tree_entries(dir: &Path, relative: &str, depth: u32, listing: &mut TreeListing) {
    if listing.truncated {
        return;
    }
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        // Unreadable directories list as empty, mirroring the scan walk.
        Err(_) => return,
    };
    let mut children: Vec<(String, WorkspaceTreeEntryKind)> = Vec::new();
    for entry in read_dir.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // `DirEntry::file_type` is symlink_metadata-based: symlinks are
        // neither followed nor listed, so a link can never escape the root.
        let kind = if file_type.is_dir() {
            if SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            WorkspaceTreeEntryKind::Dir
        } else if file_type.is_file() {
            WorkspaceTreeEntryKind::File
        } else {
            continue;
        };
        children.push((entry.file_name().to_string_lossy().into_owned(), kind));
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, kind) in children {
        if listing.entries.len() >= MAX_TREE_ENTRIES {
            listing.truncated = true;
            return;
        }
        let child_relative = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        // Push the parent before descending so a flattened depth>1 listing
        // reads parent-first, which is what a tree UI renders.
        listing.entries.push(WorkspaceTreeEntry {
            name: name.clone(),
            path: child_relative.clone(),
            kind,
        });
        if kind == WorkspaceTreeEntryKind::Dir && depth > 1 {
            fill_tree_entries(&dir.join(&name), &child_relative, depth - 1, listing);
            if listing.truncated {
                return;
            }
        }
    }
}

/// Lockfile name -> package manager id, in priority order.
pub(crate) const PACKAGE_MANAGERS: &[(&str, &str)] = &[
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("package-lock.json", "npm"),
    ("bun.lock", "bun"),
    ("bun.lockb", "bun"),
];

/// Tech id -> dependency names that signal it. First match wins per id.
const TECH_SIGNALS: &[(&str, &[&str])] = &[
    ("next", &["next"]),
    ("vite", &["vite"]),
    ("react", &["react", "react-dom"]),
    ("vue", &["vue"]),
    ("svelte", &["svelte"]),
    ("angular", &["@angular/core"]),
    ("solid", &["solid-js"]),
    ("astro", &["astro"]),
    ("express", &["express"]),
    ("nestjs", &["@nestjs/core"]),
];

/// Scan one bound root. Never fails as a whole: per-root problems land in
/// `errors`, and a root that vanished after binding produces a single
/// diagnosable error entry.
pub(crate) async fn scan_root(root_path: &str) -> WorkspaceRootDiscovery {
    let mut discovery = WorkspaceRootDiscovery {
        root_path: root_path.to_owned(),
        package_name: None,
        package_manager: None,
        tech_stack: Vec::new(),
        scripts: Vec::new(),
        sources: Vec::new(),
        git: WorkspaceGitStatus {
            is_repo: false,
            repo_root: None,
            branch: None,
            head_commit_hash: None,
            has_changes: None,
            available: false,
            error: None,
        },
        stats: WorkspaceScanStats {
            files_visited: 0,
            dirs_visited: 0,
            skipped_dirs: 0,
        },
        errors: Vec::new(),
        truncated: false,
    };

    let canonical = match fs::canonicalize(root_path) {
        Ok(path) => path,
        Err(error) => {
            discovery
                .errors
                .push(format!("root {root_path} is not accessible: {error}"));
            return discovery;
        }
    };
    if !canonical.is_dir() {
        discovery
            .errors
            .push(format!("root {root_path} is not a directory"));
        return discovery;
    }

    let mut state = ScanState::default();
    walk(&canonical, Path::new(""), 0, &mut state);
    discovery.stats = WorkspaceScanStats {
        files_visited: state.files_visited,
        dirs_visited: state.dirs_visited,
        skipped_dirs: state.skipped_dirs,
    };
    discovery.truncated = state.truncated;

    inspect_package_json(&canonical, &mut discovery);
    discovery.package_manager = detect_package_manager(&canonical);
    discovery.git = inspect_git(&canonical).await;
    discovery.sources = state.sources;
    discovery
}

#[derive(Default)]
struct ScanState {
    files_visited: u64,
    dirs_visited: u64,
    skipped_dirs: u64,
    truncated: bool,
    sources: Vec<WorkspaceSourceEntry>,
    page_count: usize,
    component_count: usize,
    route_count: usize,
}

fn walk(dir: &Path, relative_dir: &Path, depth: u32, state: &mut ScanState) {
    if state.truncated {
        return;
    }
    if depth > MAX_DEPTH {
        state.truncated = true;
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // Unreadable subdirectories are skipped; root-level inaccessibility
        // is surfaced by `scan_root` before the walk starts.
        Err(_) => return,
    };
    state.dirs_visited += 1;
    for entry in entries.flatten() {
        if state.truncated {
            return;
        }
        // `DirEntry::file_type` is symlink_metadata-based: symlinks are
        // neither followed nor counted, so a link can never escape the root.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name.as_ref()) {
                state.skipped_dirs += 1;
                continue;
            }
            walk(
                &entry.path(),
                &relative_dir.join(name.as_ref()),
                depth + 1,
                state,
            );
        } else if file_type.is_file() {
            state.files_visited += 1;
            if state.files_visited > MAX_FILES_VISITED {
                state.truncated = true;
                return;
            }
            let relative = relative_dir.join(name.as_ref());
            if let Some(source) = classify_source(&relative) {
                push_source(state, source);
            }
        }
    }
}

fn push_source(state: &mut ScanState, source: WorkspaceSourceEntry) {
    let counter = match source.kind {
        WorkspaceSourceKind::Page => &mut state.page_count,
        WorkspaceSourceKind::Component => &mut state.component_count,
        WorkspaceSourceKind::Route => &mut state.route_count,
    };
    if *counter >= MAX_SOURCES_PER_KIND {
        state.truncated = true;
        return;
    }
    *counter += 1;
    state.sources.push(source);
}

/// UI framework signal ids considered for SourceArtifact.framework.
const UI_FRAMEWORKS: &[&str] = &[
    "next", "react", "vue", "svelte", "angular", "solid", "astro",
];

/// First UI framework signal declared by the root's package.json, if any.
/// Used by the source index to tag artifacts; returns None for backend-only
/// or vanilla roots.
pub(crate) fn detect_framework(root: &Path) -> Option<String> {
    let manifest = fs::read(root.join("package.json")).ok()?;
    if manifest.len() as u64 > MAX_PACKAGE_JSON_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&manifest).ok()?;
    // Flatten declared dependency names once, then match in UI_FRAMEWORKS order.
    let mut declared: Vec<&str> = Vec::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(deps) = value.get(key).and_then(|d| d.as_object()) {
            declared.extend(deps.keys().map(String::as_str));
        }
    }
    for &fw in UI_FRAMEWORKS {
        if TECH_SIGNALS
            .iter()
            .any(|(id, names)| id == &fw && names.iter().any(|n| declared.contains(n)))
        {
            return Some(fw.to_owned());
        }
    }
    None
}

/// Classify one visited file (path relative to the scanned root) against
/// directory conventions. Returns `None` for files that match no convention
/// (most source files).
pub(crate) fn classify_source(relative: &Path) -> Option<WorkspaceSourceEntry> {
    let extension = relative.extension()?.to_string_lossy();
    if !SOURCE_EXTENSIONS.contains(&extension.as_ref()) {
        return None;
    }
    let parts: Vec<String> = relative
        .iter()
        .map(|part| part.to_string_lossy().into_owned())
        .collect();
    let file = parts.last()?.clone();
    let stem = file
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_owned())
        .unwrap_or_else(|| file.clone());
    let dirs = &parts[..parts.len().saturating_sub(1)];
    let rel_path = parts.join("/");

    // Next.js app router: `app/**/page.*` or `src/app/**/page.*`.
    if stem == "page"
        && let Some(app_index) = find_convention(dirs, &["app", "src/app"])
    {
        return Some(WorkspaceSourceEntry {
            kind: WorkspaceSourceKind::Route,
            name: stem,
            path: rel_path,
            route_path: Some(route_path_from_app_dirs(&dirs[app_index + 1..])),
        });
    }

    // Next.js pages router: `pages/**` or `src/pages/**`. Route files are
    // `index.*` or lowercase stems (Next convention); PascalCase files are
    // page-level components and fall through to the generic Page bucket.
    if let Some(pages_index) = find_convention(dirs, &["pages", "src/pages"])
        && (stem == "index" || stem.chars().next().is_some_and(|c| c.is_lowercase()))
    {
        return Some(WorkspaceSourceEntry {
            kind: WorkspaceSourceKind::Route,
            name: stem.clone(),
            path: rel_path.clone(),
            route_path: Some(route_path_from_pages(&dirs[pages_index + 1..], &stem)),
        });
    }

    // Generic routes directories.
    if find_convention(dirs, &["routes", "src/routes"]).is_some() {
        let route_path = {
            let segments: Vec<&str> = dirs
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(stem.as_str()))
                .collect();
            format!("/{}", segments.join("/"))
        };
        return Some(WorkspaceSourceEntry {
            kind: WorkspaceSourceKind::Route,
            name: stem,
            path: rel_path,
            route_path: Some(route_path),
        });
    }

    // Generic pages/views directories.
    if find_convention(dirs, &["pages", "src/pages", "views", "src/views"]).is_some() {
        return Some(WorkspaceSourceEntry {
            kind: WorkspaceSourceKind::Page,
            name: stem,
            path: rel_path,
            route_path: None,
        });
    }

    // Components at any directory depth.
    if dirs.iter().any(|dir| dir == "components") {
        return Some(WorkspaceSourceEntry {
            kind: WorkspaceSourceKind::Component,
            name: stem,
            path: rel_path,
            route_path: None,
        });
    }

    // Vite/standard templates keep the root component directly under src/
    // (`src/App.tsx`) with no components/ wrapper. Only PascalCase stems
    // without dots: lowercase entry files (`src/main.tsx`) and test files
    // (`src/App.test.tsx`) resolve as symbol noise, not pick targets.
    if dirs.len() == 1
        && dirs[0] == "src"
        && !stem.contains('.')
        && stem.chars().next().is_some_and(|c| c.is_uppercase())
    {
        return Some(WorkspaceSourceEntry {
            kind: WorkspaceSourceKind::Page,
            name: stem,
            path: rel_path,
            route_path: None,
        });
    }

    None
}

/// First index where `dirs` contains one of the `/`-joined conventions.
fn find_convention(dirs: &[String], conventions: &[&str]) -> Option<usize> {
    conventions.iter().find_map(|convention| {
        let wanted: Vec<&str> = convention.split('/').collect();
        dirs.windows(wanted.len())
            .position(|window| window.iter().map(String::as_str).eq(wanted.iter().copied()))
    })
}

/// `/` for `app/page.tsx`, `/blog/[slug]` for `app/blog/[slug]/page.tsx`.
/// Route groups `(marketing)` and private `_segments` do not affect the path.
fn route_path_from_app_dirs(segments: &[String]) -> String {
    let mut route = String::new();
    for segment in segments {
        if (segment.starts_with('(') && segment.ends_with(')')) || segment.starts_with('_') {
            continue;
        }
        route.push('/');
        route.push_str(segment);
    }
    if route.is_empty() {
        "/".to_owned()
    } else {
        route
    }
}

/// `/` for `pages/index.tsx`, `/about` for `pages/about.tsx`,
/// `/blog/[id]` for `pages/blog/[id].tsx`.
fn route_path_from_pages(segments: &[String], stem: &str) -> String {
    let mut parts: Vec<&str> = segments.iter().map(String::as_str).collect();
    if stem != "index" {
        parts.push(stem);
    }
    let route = parts.join("/");
    if route.is_empty() {
        "/".to_owned()
    } else {
        format!("/{route}")
    }
}

pub(crate) fn inspect_package_json(root: &Path, discovery: &mut WorkspaceRootDiscovery) {
    let path = root.join("package.json");
    let Ok(metadata) = fs::metadata(&path) else {
        return; // no manifest: fine for backend or non-JS roots
    };
    if !metadata.is_file() {
        return;
    }
    if metadata.len() > MAX_PACKAGE_JSON_BYTES {
        discovery.errors.push(format!(
            "package.json exceeds {MAX_PACKAGE_JSON_BYTES} bytes and was skipped"
        ));
        return;
    }
    let Ok(raw) = fs::read_to_string(&path) else {
        discovery.errors.push(format!(
            "package.json at {} is not readable",
            path.display()
        ));
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        discovery.errors.push(format!(
            "package.json at {} is not valid JSON",
            path.display()
        ));
        return;
    };
    discovery.package_name = value
        .get("name")
        .and_then(|name| name.as_str())
        .map(str::to_owned);
    if let Some(scripts) = value.get("scripts").and_then(|v| v.as_object()) {
        let mut scripts = scripts
            .iter()
            .map(|(name, command)| WorkspaceScript {
                name: name.clone(),
                command: command.as_str().unwrap_or_default().to_owned(),
            })
            .collect::<Vec<_>>();
        scripts.sort_by(|a, b| a.name.cmp(&b.name));
        discovery.scripts = scripts;
    }
    let mut signals: BTreeMap<String, Option<String>> = BTreeMap::new();
    for field in ["dependencies", "devDependencies"] {
        let Some(deps) = value.get(field).and_then(|v| v.as_object()) else {
            continue;
        };
        for (id, needles) in TECH_SIGNALS {
            for needle in *needles {
                if let Some(version) = deps.get(*needle).and_then(|v| v.as_str()) {
                    signals
                        .entry((*id).to_owned())
                        .or_insert_with(|| Some(version.to_owned()));
                }
            }
        }
    }
    discovery.tech_stack = signals
        .into_iter()
        .map(|(id, version)| WorkspaceTechEntry { id, version })
        .collect();
}

pub(crate) fn detect_package_manager(root: &Path) -> Option<String> {
    PACKAGE_MANAGERS
        .iter()
        .find(|(lockfile, _)| root.join(lockfile).is_file())
        .map(|(_, manager)| (*manager).to_owned())
}

async fn inspect_git(root: &Path) -> WorkspaceGitStatus {
    let mut status = WorkspaceGitStatus {
        is_repo: false,
        repo_root: None,
        branch: None,
        head_commit_hash: None,
        has_changes: None,
        available: false,
        error: None,
    };
    let Some(repo_root) = ody_git_utils::get_git_repo_root(root) else {
        return status; // not a repository: nothing diagnosable
    };
    status.is_repo = true;
    status.repo_root = Some(repo_root.to_string_lossy().into_owned());
    match ody_git_utils::collect_git_info(root).await {
        Some(info) => {
            status.available = true;
            status.branch = info.branch;
            status.head_commit_hash = info.commit_hash.map(|sha| sha.0);
        }
        None => {
            status.error = Some(
                "git repository detected but info collection failed \
                 (git binary missing, errored, or timed out)"
                    .to_owned(),
            );
        }
    }
    if status.available {
        status.has_changes = ody_git_utils::get_has_changes(root).await;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("create fixture root")
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent dir")).expect("create parent");
        fs::write(path, contents).expect("write fixture file");
    }

    fn package_json(name: &str, deps: &str, dev_deps: &str, scripts: &str) -> String {
        format!(
            r#"{{
  "name": "{name}",
  "scripts": {{ {scripts} }},
  "dependencies": {{ {deps} }},
  "devDependencies": {{ {dev_deps} }}
}}"#
        )
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<String, u64> {
        let mut snapshot = BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read dir").flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    stack.push(path);
                } else if file_type.is_file() {
                    let relative = path
                        .strip_prefix(root)
                        .expect("relative")
                        .to_string_lossy()
                        .into_owned();
                    let hash = fs::read(&path)
                        .expect("read file")
                        .iter()
                        .fold(0xcbf29ce484222325u64, |acc, byte| {
                            (acc ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                        });
                    snapshot.insert(relative, hash);
                }
            }
        }
        snapshot
    }

    #[tokio::test]
    async fn detects_vite_react_stack_pages_components_and_pnpm() {
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json(
                "storefront",
                r#""react": "^18.2.0", "react-dom": "^18.2.0""#,
                r#""vite": "^5.0.0""#,
                r#""dev": "vite", "build": "vite build", "test": "vitest""#,
            ),
        );
        write(&root.path(), "pnpm-lock.yaml", "lockfileVersion: 9\n");
        write(&root.path(), "index.html", "<html></html>");
        write(&root.path(), "src/main.tsx", "export {}");
        write(&root.path(), "src/pages/Home.tsx", "export {}");
        write(&root.path(), "src/pages/About.tsx", "export {}");
        write(&root.path(), "src/components/Button.tsx", "export {}");
        write(
            &root.path(),
            "node_modules/left-pad/index.js",
            "module.exports = {}",
        );

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        assert!(discovery.errors.is_empty(), "{:?}", discovery.errors);
        assert!(!discovery.truncated);
        assert_eq!(discovery.package_name.as_deref(), Some("storefront"));
        assert_eq!(discovery.package_manager.as_deref(), Some("pnpm"));
        let tech: BTreeMap<String, Option<String>> = discovery
            .tech_stack
            .iter()
            .map(|entry| (entry.id.clone(), entry.version.clone()))
            .collect();
        assert_eq!(
            tech.get("react").and_then(|v| v.as_deref()),
            Some("^18.2.0")
        );
        assert!(tech.contains_key("vite"));
        let script_names: Vec<&str> = discovery.scripts.iter().map(|s| s.name.as_str()).collect();
        assert!(script_names.contains(&"dev"));
        assert!(script_names.contains(&"build"));
        assert!(script_names.contains(&"test"));

        let pages: Vec<&str> = discovery
            .sources
            .iter()
            .filter(|s| s.kind == WorkspaceSourceKind::Page)
            .map(|s| s.path.as_str())
            .collect();
        assert!(pages.contains(&"src/pages/Home.tsx"));
        assert!(pages.contains(&"src/pages/About.tsx"));
        let components: Vec<&str> = discovery
            .sources
            .iter()
            .filter(|s| s.kind == WorkspaceSourceKind::Component)
            .map(|s| s.path.as_str())
            .collect();
        assert_eq!(components, vec!["src/components/Button.tsx"]);
        // node_modules is skipped entirely.
        assert!(
            !discovery
                .sources
                .iter()
                .any(|s| s.path.contains("node_modules"))
        );
        assert!(discovery.stats.skipped_dirs >= 1);
    }

    #[tokio::test]
    async fn detects_next_app_router_routes() {
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json(
                "blog",
                r#""next": "14.2.0", "react": "18.2.0""#,
                "",
                r#""dev": "next dev""#,
            ),
        );
        write(
            &root.path(),
            "app/page.tsx",
            "export default function Page() {}",
        );
        write(
            &root.path(),
            "app/blog/[slug]/page.tsx",
            "export default function Page() {}",
        );
        write(
            &root.path(),
            "app/(marketing)/about/page.tsx",
            "export default function Page() {}",
        );
        write(&root.path(), "app/api/health/route.ts", "export {}");

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        let routes: BTreeMap<String, String> = discovery
            .sources
            .iter()
            .filter(|s| s.kind == WorkspaceSourceKind::Route)
            .filter_map(|s| s.route_path.clone().map(|rp| (s.path.clone(), rp)))
            .collect();
        assert_eq!(routes.get("app/page.tsx").map(String::as_str), Some("/"));
        assert_eq!(
            routes.get("app/blog/[slug]/page.tsx").map(String::as_str),
            Some("/blog/[slug]")
        );
        // Route groups do not affect the route path.
        assert_eq!(
            routes
                .get("app/(marketing)/about/page.tsx")
                .map(String::as_str),
            Some("/about")
        );
        let tech: Vec<&str> = discovery.tech_stack.iter().map(|t| t.id.as_str()).collect();
        assert!(tech.contains(&"next"));
        assert!(tech.contains(&"react"));
    }

    #[tokio::test]
    async fn detects_pages_router_and_vue_views() {
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json("legacy", r#""vue": "^3.4.0""#, "", r#""dev": "vite""#),
        );
        write(&root.path(), "src/pages/index.tsx", "export {}");
        write(&root.path(), "src/pages/about.tsx", "export {}");
        write(
            &root.path(),
            "src/views/Dashboard.vue",
            "<template></template>",
        );
        write(
            &root.path(),
            "src/components/Nav.vue",
            "<template></template>",
        );

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        let routes: BTreeMap<String, String> = discovery
            .sources
            .iter()
            .filter(|s| s.kind == WorkspaceSourceKind::Route)
            .filter_map(|s| s.route_path.clone().map(|rp| (s.path.clone(), rp)))
            .collect();
        assert_eq!(
            routes.get("src/pages/index.tsx").map(String::as_str),
            Some("/")
        );
        assert_eq!(
            routes.get("src/pages/about.tsx").map(String::as_str),
            Some("/about")
        );
        let pages: Vec<&str> = discovery
            .sources
            .iter()
            .filter(|s| s.kind == WorkspaceSourceKind::Page)
            .map(|s| s.path.as_str())
            .collect();
        assert_eq!(pages, vec!["src/views/Dashboard.vue"]);
        let tech: Vec<&str> = discovery.tech_stack.iter().map(|t| t.id.as_str()).collect();
        assert!(tech.contains(&"vue"));
    }

    #[tokio::test]
    async fn classifies_pascalcase_src_root_files_as_pages() {
        // Vite/standard templates keep the root component at `src/App.tsx`
        // (no components/ wrapper); without this convention the most
        // picked element of such projects cannot be symbol-resolved.
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json(
                "vite-template",
                r#""react": "^18.3.0", "react-dom": "^18.3.0""#,
                r#""vite": "^5.0.0""#,
                r#""dev": "vite""#,
            ),
        );
        write(&root.path(), "src/main.tsx", "export {}");
        write(&root.path(), "src/App.tsx", "export {}");
        write(&root.path(), "src/App.test.tsx", "export {}");

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        let pages: Vec<&str> = discovery
            .sources
            .iter()
            .filter(|s| s.kind == WorkspaceSourceKind::Page)
            .map(|s| s.path.as_str())
            .collect();
        assert_eq!(pages, vec!["src/App.tsx"]);
    }

    #[tokio::test]
    async fn backend_root_without_pages_scans_cleanly() {
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json(
                "api",
                r#""express": "^4.19.0""#,
                "",
                r#""start": "node server.js""#,
            ),
        );
        write(&root.path(), "server.js", "console.log('ok')");
        write(&root.path(), "package-lock.json", "{}");

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        assert!(discovery.errors.is_empty(), "{:?}", discovery.errors);
        assert_eq!(discovery.package_manager.as_deref(), Some("npm"));
        let tech: Vec<&str> = discovery.tech_stack.iter().map(|t| t.id.as_str()).collect();
        assert!(tech.contains(&"express"));
        assert!(discovery.sources.is_empty());
    }

    #[tokio::test]
    async fn truncates_when_source_cap_exceeded() {
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json("big", "", "", ""),
        );
        for index in 0..600 {
            write(
                &root.path(),
                &format!("src/components/Widget{index}.tsx"),
                "export {}",
            );
        }

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        assert!(discovery.truncated);
        let component_count = discovery
            .sources
            .iter()
            .filter(|s| s.kind == WorkspaceSourceKind::Component)
            .count();
        assert_eq!(component_count, MAX_SOURCES_PER_KIND);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn does_not_follow_escaping_symlinks() {
        let root = fixture_root();
        let outside = fixture_root();
        write(&outside.path(), "secret.ts", "export {}");
        write(
            &root.path(),
            "package.json",
            &package_json("links", "", "", ""),
        );
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked-outside"))
            .expect("create symlink");

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        assert!(!discovery.sources.iter().any(|s| s.path.contains("secret")));
        assert_eq!(discovery.stats.files_visited, 1); // only package.json
    }

    #[tokio::test]
    async fn honors_depth_limit() {
        let root = fixture_root();
        let deep = (0..20).map(|_| "deep").collect::<Vec<_>>().join("/");
        write(&root.path(), &format!("{deep}/bottom.ts"), "export {}");

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        assert!(discovery.truncated);
        assert!(
            !discovery
                .sources
                .iter()
                .any(|s| s.path.contains("bottom.ts"))
        );
    }

    #[tokio::test]
    async fn missing_root_produces_diagnosable_error() {
        let discovery = scan_root("/definitely/not/a/real/path-xyzzy").await;
        assert_eq!(discovery.errors.len(), 1);
        assert!(
            discovery.errors[0].contains("/definitely/not/a/real/path-xyzzy"),
            "{}",
            discovery.errors[0]
        );
    }

    #[tokio::test]
    async fn corrupt_package_json_reports_error_and_continues() {
        let root = fixture_root();
        write(&root.path(), "package.json", "{ not json");

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        assert_eq!(discovery.errors.len(), 1);
        assert!(discovery.errors[0].contains("package.json"));
        assert_eq!(discovery.package_name, None);
    }

    #[tokio::test]
    async fn fake_git_dir_detected_without_git_binary() {
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json("repo", "", "", ""),
        );
        fs::create_dir(root.path().join(".git")).expect("create fake .git");

        let discovery = scan_root(&root.path().to_string_lossy()).await;

        assert!(discovery.git.is_repo);
        let canonical_root = root.path().canonicalize().expect("canonical");
        assert_eq!(discovery.git.repo_root.as_deref(), canonical_root.to_str());
        // A fake .git makes git commands fail even when the binary exists.
        assert!(!discovery.git.available);
        assert!(discovery.git.error.is_some());
        assert!(discovery.stats.skipped_dirs >= 1); // .git skipped in walk
    }

    #[tokio::test]
    async fn real_git_repo_reports_branch_head_and_dirty_state() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("git binary unavailable; skipping real-repo assertions");
            return;
        }
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json("repo", "", "", ""),
        );
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(root.path())
                .output()
                .expect("run git")
        };
        assert!(run_git(&["init"]).status.success());
        write(&root.path(), "file.txt", "hello");
        assert!(run_git(&["add", "."]).status.success());
        assert!(
            run_git(&[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Test",
                "commit",
                "-m",
                "init",
            ])
            .status
            .success()
        );

        let clean = scan_root(&root.path().to_string_lossy()).await;
        assert!(clean.git.available, "{:?}", clean.git.error);
        assert!(
            matches!(clean.git.branch.as_deref(), Some("master") | Some("main")),
            "unexpected default branch: {:?}",
            clean.git.branch
        );
        assert!(clean.git.head_commit_hash.is_some());
        assert_eq!(clean.git.has_changes, Some(false));

        write(&root.path(), "untracked.txt", "dirty");
        let dirty = scan_root(&root.path().to_string_lossy()).await;
        assert_eq!(dirty.git.has_changes, Some(true));
    }

    #[tokio::test]
    async fn scan_does_not_modify_any_file_under_root() {
        let root = fixture_root();
        write(
            &root.path(),
            "package.json",
            &package_json("readonly", r#""react": "18.2.0""#, "", r#""dev": "vite""#),
        );
        write(&root.path(), "src/pages/Home.tsx", "export {}");
        write(&root.path(), "src/components/Button.tsx", "export {}");
        let before = snapshot_tree(root.path());

        let _ = scan_root(&root.path().to_string_lossy()).await;

        assert_eq!(before, snapshot_tree(root.path()));
    }
}

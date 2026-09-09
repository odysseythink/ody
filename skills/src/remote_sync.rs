//! Remote-synced builtin skills (odyBox port of chatbox's `builtin-sync.ts`).
//!
//! Fetches a builtin-skill manifest plus per-skill details from a remote
//! backend and mirrors them into `$ODY_HOME/skills/.builtin` so the unified
//! skill registry discovers them as system-scope skills. Synced skills are
//! tagged with `policy.products` so they are only visible to the products that
//! own them (e.g. odyBox) regardless of which home they land in.
//!
//! Fail-open by contract: any network or parse failure keeps the existing
//! local snapshot untouched, so builtin skills never disappear because the
//! backend is down.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;

use ody_protocol::protocol::Product;
use ody_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

const REMOTE_SKILLS_DIR_NAME: &str = "skills";
const REMOTE_BUILTIN_DIR_NAME: &str = ".builtin";
const MANIFEST_FILE_NAME: &str = "manifest.json";
const SKILL_MD_FILE_NAME: &str = "SKILL.md";
const METADATA_DIR_NAME: &str = "agents";
const METADATA_FILE_NAME: &str = "odysseythink.yaml";
const MAX_SKILL_NAME_LEN: usize = 64;

/// Remote skills that must never be mirrored into `.builtin`.
/// `chatbox-product-info` is chatbox product marketing; odyBox is an
/// independent product and ships its own product info instead.
pub const REMOTE_SYNC_DENYLIST: &[&str] = &["chatbox-product-info"];

/// On-disk cache location for remote-synced builtin skills.
pub fn remote_builtin_cache_root_dir(ody_home: &AbsolutePathBuf) -> AbsolutePathBuf {
    ody_home
        .join(REMOTE_SKILLS_DIR_NAME)
        .join(REMOTE_BUILTIN_DIR_NAME)
}

#[derive(Clone, Debug, Deserialize)]
pub struct RemoteManifestItem {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<u64>,
    pub hash: String,
    #[serde(default, rename = "updated_at")]
    pub updated_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RemoteSkillDetail {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub body: String,
    #[serde(default)]
    pub files: Option<Vec<RemoteSkillFile>>,
    #[serde(default)]
    pub version: Option<u64>,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default, rename = "allowed_tools")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub license: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RemoteSkillFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RemoteSnapshotManifest {
    #[serde(default)]
    pub skills: BTreeMap<String, SnapshotEntry>,
    #[serde(default, rename = "syncedAt")]
    pub synced_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub version: u64,
    pub hash: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    pub origin: SnapshotOrigin,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotOrigin {
    Seed,
    Remote,
}

/// Which products the synced skills are tagged for. Synced builtin skills are
/// product-owned content (e.g. chatbox backend skills belong to odyBox), so
/// the tag keeps them hidden from other product restrictions.
pub struct RemoteSyncOptions {
    pub products: Vec<Product>,
}

#[derive(Debug, Error)]
pub enum RemoteSyncError {
    #[error("io error while {action}: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("fetch failed: {0}")]
    Fetch(String),
}

impl RemoteSyncError {
    fn io(action: &'static str, source: std::io::Error) -> Self {
        Self::Io { action, source }
    }
}

/// Backend access. Implemented with a real HTTP client in the app-server and
/// with fakes in tests.
#[allow(async_fn_in_trait)]
pub trait RemoteSkillFetcher {
    async fn fetch_manifest(&self) -> Result<Vec<RemoteManifestItem>, RemoteSyncError>;
    async fn fetch_detail(&self, name: &str) -> Result<Option<RemoteSkillDetail>, RemoteSyncError>;
}

fn is_valid_remote_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SKILL_NAME_LEN
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

/// Validates a remote-relative file path and returns its normalized
/// slash-separated form. Rejects absolute paths, parent-dir traversal, the
/// generated `SKILL.md`, and reserved `source.json` (case-insensitive).
fn validate_remote_file_path(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return None;
    }
    let mut normalized = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part.to_string_lossy().to_string()),
            _ => return None,
        }
    }
    if normalized.is_empty() {
        return None;
    }
    if normalized.len() == 1 {
        let lower = normalized[0].to_lowercase();
        if lower == "skill.md" || lower == "source.json" {
            return None;
        }
    }
    Some(normalized.join("/"))
}

fn manifest_path(root: &AbsolutePathBuf) -> AbsolutePathBuf {
    root.join(MANIFEST_FILE_NAME)
}

fn read_manifest(root: &AbsolutePathBuf) -> RemoteSnapshotManifest {
    let Ok(raw) = fs::read_to_string(manifest_path(root).as_path()) else {
        return RemoteSnapshotManifest::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn write_manifest(
    root: &AbsolutePathBuf,
    manifest: &RemoteSnapshotManifest,
) -> Result<(), RemoteSyncError> {
    fs::create_dir_all(root.as_path()).map_err(|e| RemoteSyncError::io("create snapshot root", e))?;
    let serialized = serde_json::to_string_pretty(manifest)
        .map_err(|e| RemoteSyncError::Fetch(format!("serialize manifest: {e}")))?;
    fs::write(manifest_path(root).as_path(), serialized)
        .map_err(|e| RemoteSyncError::io("write manifest", e))
}

/// Writes one synced skill: SKILL.md (frontmatter + body), validated
/// attachment files, and the product-gate metadata file. Replaces any
/// previous directory content.
fn write_synced_skill(
    root: &AbsolutePathBuf,
    name: &str,
    detail: &RemoteSkillDetail,
    options: &RemoteSyncOptions,
) -> Result<(), RemoteSyncError> {
    let skill_dir = root.join(name);
    if skill_dir.as_path().exists() {
        fs::remove_dir_all(skill_dir.as_path())
            .map_err(|e| RemoteSyncError::io("replace synced skill dir", e))?;
    }
    fs::create_dir_all(skill_dir.as_path())
        .map_err(|e| RemoteSyncError::io("create synced skill dir", e))?;

    let mut frontmatter = BTreeMap::new();
    frontmatter.insert("name".to_string(), detail.name.clone());
    frontmatter.insert(
        "description".to_string(),
        detail.description.clone().unwrap_or_default(),
    );
    if let Some(license) = &detail.license {
        frontmatter.insert("license".to_string(), license.clone());
    }
    let fm_yaml = serde_yaml::to_string(&frontmatter)
        .map_err(|e| RemoteSyncError::Fetch(format!("serialize frontmatter: {e}")))?;
    let skill_md = format!("---\n{fm_yaml}---\n\n{}\n", detail.body.trim());
    fs::write(skill_dir.join(SKILL_MD_FILE_NAME).as_path(), skill_md)
        .map_err(|e| RemoteSyncError::io("write SKILL.md", e))?;

    let mut seen = BTreeSet::new();
    for file in detail.files.as_deref().unwrap_or_default() {
        let Some(normalized) = validate_remote_file_path(&file.path) else {
            tracing::warn!(
                "synced skill {name}: skipping invalid file path {:?}",
                file.path
            );
            continue;
        };
        if !seen.insert(normalized.clone()) {
            tracing::warn!("synced skill {name}: duplicate file path {normalized}");
            continue;
        }
        let target = skill_dir.join(&normalized);
        if let Some(parent) = target.as_path().parent() {
            fs::create_dir_all(parent)
                .map_err(|e| RemoteSyncError::io("create attachment parent dir", e))?;
        }
        fs::write(target.as_path(), &file.content)
            .map_err(|e| RemoteSyncError::io("write attachment file", e))?;
    }

    if !options.products.is_empty() {
        let metadata_dir = skill_dir.join(METADATA_DIR_NAME);
        fs::create_dir_all(metadata_dir.as_path())
            .map_err(|e| RemoteSyncError::io("create metadata dir", e))?;
        let mut yaml = String::from("policy:\n  products:\n");
        for product in &options.products {
            yaml.push_str(&format!("    - {}\n", product.to_app_platform()));
        }
        fs::write(metadata_dir.join(METADATA_FILE_NAME).as_path(), yaml)
            .map_err(|e| RemoteSyncError::io("write product metadata", e))?;
    }

    Ok(())
}

/// Syncs remote builtin skills into `$ODY_HOME/skills/.builtin`.
///
/// Fail-open: a manifest fetch failure returns `Ok(false)` and leaves the
/// local snapshot untouched. Per-skill problems are logged and skipped.
/// Returns `Ok(true)` when at least one skill snapshot changed.
pub async fn sync_remote_builtin_skills(
    ody_home: &AbsolutePathBuf,
    fetcher: &impl RemoteSkillFetcher,
    options: &RemoteSyncOptions,
    skip: &[String],
) -> Result<bool, RemoteSyncError> {
    let root = remote_builtin_cache_root_dir(ody_home);
    fs::create_dir_all(root.as_path()).map_err(|e| RemoteSyncError::io("create snapshot root", e))?;

    let remote_items = match fetcher.fetch_manifest().await {
        Ok(items) => items,
        Err(err) => {
            tracing::warn!("remote builtin skills sync skipped: {err}");
            return Ok(false);
        }
    };

    let mut manifest = read_manifest(&root);
    let mut changed = false;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();

    for item in &remote_items {
        if skip.iter().any(|skipped| skipped == &item.name)
            || REMOTE_SYNC_DENYLIST.contains(&item.name.as_str())
        {
            continue;
        }
        if !is_valid_remote_skill_name(&item.name) {
            tracing::warn!(
                "remote builtin skills sync: skipping invalid skill name {:?}",
                item.name
            );
            continue;
        }
        if manifest
            .skills
            .get(&item.name)
            .is_some_and(|entry| entry.hash == item.hash)
        {
            continue;
        }
        let Ok(Some(detail)) = fetcher.fetch_detail(&item.name).await else {
            tracing::warn!(
                "remote builtin skills sync: no usable detail for {:?}",
                item.name
            );
            continue;
        };
        if detail.body.trim().is_empty() {
            tracing::warn!(
                "remote builtin skills sync: empty body for {:?}",
                item.name
            );
            continue;
        }
        match write_synced_skill(&root, &item.name, &detail, options) {
            Ok(()) => {
                manifest.skills.insert(
                    item.name.clone(),
                    SnapshotEntry {
                        version: detail.version.or(item.version).unwrap_or(1),
                        hash: item.hash.clone(),
                        updated_at: now,
                        origin: SnapshotOrigin::Remote,
                    },
                );
                changed = true;
                tracing::info!("remote builtin skills sync: updated {:?}", item.name);
            }
            Err(err) => {
                tracing::warn!(
                    "remote builtin skills sync: failed writing {:?}: {err}",
                    item.name
                );
            }
        }
    }

    // Remove snapshots whose names are now skipped or denylisted so stale
    // copies from earlier syncs (or seeds shadowed by the backend) do not
    // linger and surface as duplicate skills.
    for name in skip.iter().map(String::as_str).chain(REMOTE_SYNC_DENYLIST.iter().copied()) {
        let dir = root.join(name);
        let removed_disk = dir.as_path().exists() && fs::remove_dir_all(&dir).is_ok();
        let removed_manifest = manifest.skills.remove(name).is_some();
        if removed_disk || removed_manifest {
            changed = true;
        }
    }

    manifest.synced_at = now;
    write_manifest(&root, &manifest)?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::RemoteManifestItem;
    use super::RemoteSkillDetail;
    use super::RemoteSkillFetcher;
    use super::RemoteSyncError;
    use super::RemoteSyncOptions;
    use super::remote_builtin_cache_root_dir;
    use super::sync_remote_builtin_skills;
    use super::validate_remote_file_path;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct FakeFetcher {
        manifest: Vec<RemoteManifestItem>,
        details: HashMap<String, RemoteSkillDetail>,
        fail_manifest: bool,
    }

    impl FakeFetcher {
        fn new(items: Vec<(&str, &str, &str)>) -> Self {
            Self {
                manifest: items
                    .iter()
                    .map(|(name, hash, _body)| RemoteManifestItem {
                        name: name.to_string(),
                        description: None,
                        version: Some(1),
                        hash: hash.to_string(),
                        updated_at: None,
                    })
                    .collect(),
                details: items
                    .iter()
                    .map(|(name, _hash, body)| {
                        (
                            name.to_string(),
                            RemoteSkillDetail {
                                name: name.to_string(),
                                description: Some(format!("description of {name}")),
                                body: body.to_string(),
                                files: None,
                                version: Some(1),
                                hash: None,
                                allowed_tools: None,
                                metadata: None,
                                license: None,
                            },
                        )
                    })
                    .collect(),
                fail_manifest: false,
            }
        }

        fn detail(mut self, name: &str, detail: RemoteSkillDetail) -> Self {
            self.details.insert(name.to_string(), detail);
            self
        }
    }

    #[allow(async_fn_in_trait)]
    impl RemoteSkillFetcher for FakeFetcher {
        async fn fetch_manifest(&self) -> Result<Vec<RemoteManifestItem>, RemoteSyncError> {
            if self.fail_manifest {
                return Err(RemoteSyncError::Fetch("offline".to_string()));
            }
            Ok(self.manifest.clone())
        }

        async fn fetch_detail(
            &self,
            name: &str,
        ) -> Result<Option<RemoteSkillDetail>, RemoteSyncError> {
            Ok(self.details.get(name).cloned())
        }
    }

    fn ody_home() -> (tempfile::TempDir, ody_utils_absolute_path::AbsolutePathBuf) {
        let temp_dir = tempfile::tempdir().unwrap();
        let home = ody_utils_absolute_path::AbsolutePathBuf::try_from(temp_dir.path())
            .expect("absolute temp dir");
        (temp_dir, home)
    }

    fn skill_md(root: &ody_utils_absolute_path::AbsolutePathBuf, name: &str) -> String {
        std::fs::read_to_string(root.join(name).join("SKILL.md").as_path()).unwrap()
    }

    #[tokio::test]
    async fn syncs_new_skills_and_is_incremental() {
        let (_tmp, home) = ody_home();
        let options = RemoteSyncOptions { products: vec![] };
        let fetcher = FakeFetcher::new(vec![
            ("a-stock-data", "hash-a", "body A"),
            ("humanizer-zh", "hash-h", "body H"),
        ]);

        let changed = sync_remote_builtin_skills(&home, &fetcher, &options, &[])
            .await
            .unwrap();
        assert!(changed, "first sync must report changes");

        let root = remote_builtin_cache_root_dir(&home);
        let md = skill_md(&root, "a-stock-data");
        assert!(md.contains("name: a-stock-data"), "frontmatter: {md}");
        assert!(md.contains("body A"), "body: {md}");
        assert!(std::path::Path::new(&root.join("humanizer-zh").join("SKILL.md").as_path()).exists());

        let changed_again = sync_remote_builtin_skills(&home, &fetcher, &options, &[])
            .await
            .unwrap();
        assert!(!changed_again, "unchanged manifest must be a no-op");
    }

    #[tokio::test]
    async fn rewrites_only_the_changed_skill() {
        let (_tmp, home) = ody_home();
        let options = RemoteSyncOptions { products: vec![] };
        let initial = FakeFetcher::new(vec![
            ("skill-one", "hash-1", "v1 body"),
            ("skill-two", "hash-2", "two body"),
        ]);
        sync_remote_builtin_skills(&home, &initial, &options, &[])
            .await
            .unwrap();

        let updated = FakeFetcher::new(vec![
            ("skill-one", "hash-1b", "v2 body"),
            ("skill-two", "hash-2", "two body"),
        ]);
        let changed = sync_remote_builtin_skills(&home, &updated, &options, &[])
            .await
            .unwrap();

        let root = remote_builtin_cache_root_dir(&home);
        assert!(changed);
        assert!(skill_md(&root, "skill-one").contains("v2 body"));
        assert!(skill_md(&root, "skill-two").contains("two body"));
    }

    #[tokio::test]
    async fn skips_invalid_skill_names() {
        let (_tmp, home) = ody_home();
        let options = RemoteSyncOptions { products: vec![] };
        let fetcher = FakeFetcher::new(vec![
            ("../evil", "hash-e", "evil body"),
            ("Bad Name", "hash-b", "bad body"),
            ("", "hash-empty", "empty body"),
            ("good-name", "hash-g", "good body"),
        ]);

        let changed = sync_remote_builtin_skills(&home, &fetcher, &options, &[])
            .await
            .unwrap();

        let root = remote_builtin_cache_root_dir(&home);
        assert!(changed, "the one valid skill should sync");
        assert!(std::path::Path::new(&root.join("good-name").join("SKILL.md").as_path()).exists());
        assert!(!root.join("..").join("evil").as_path().exists());
        assert!(!root.as_path().join("Bad Name").exists());
        let manifest = std::fs::read_to_string(root.join("manifest.json").as_path()).unwrap();
        assert!(!manifest.contains("evil"));
        assert!(!manifest.contains("Bad Name"));
    }

    #[tokio::test]
    async fn skips_denylisted_and_seed_covered_skills() {
        let (_tmp, home) = ody_home();
        let options = RemoteSyncOptions { products: vec![] };
        let fetcher = FakeFetcher::new(vec![
            ("chatbox-product-info", "hash-c", "chatbox marketing"),
            ("data-analysis", "hash-d", "backend copy"),
            ("a-stock-data", "hash-a", "body A"),
        ]);
        let skip = vec!["data-analysis".to_string()];

        let changed = sync_remote_builtin_skills(&home, &fetcher, &options, &skip)
            .await
            .unwrap();

        let root = remote_builtin_cache_root_dir(&home);
        assert!(changed, "a-stock-data should sync");
        assert!(std::path::Path::new(&root.join("a-stock-data").join("SKILL.md").as_path()).exists());
        assert!(!root.join("chatbox-product-info").as_path().exists());
        assert!(!root.join("data-analysis").as_path().exists());
        let manifest = std::fs::read_to_string(root.join("manifest.json").as_path()).unwrap();
        assert!(!manifest.contains("chatbox-product-info"));
        assert!(!manifest.contains("data-analysis"));
    }

    #[tokio::test]
    async fn removes_stale_snapshots_for_skipped_names() {
        let (_tmp, home) = ody_home();
        let options = RemoteSyncOptions { products: vec![] };
        let initial = FakeFetcher::new(vec![("data-analysis", "hash-d", "backend copy")]);
        sync_remote_builtin_skills(&home, &initial, &options, &[])
            .await
            .unwrap();
        let root = remote_builtin_cache_root_dir(&home);
        // Simulate a stale snapshot from before the denylist existed.
        std::fs::create_dir_all(root.join("chatbox-product-info").as_path()).unwrap();
        assert!(root.join("data-analysis").as_path().exists());

        // Next sync skips both names: stale snapshots must be removed.
        let empty = FakeFetcher::new(vec![]);
        let skip = vec!["data-analysis".to_string()];
        let changed = sync_remote_builtin_skills(&home, &empty, &options, &skip)
            .await
            .unwrap();

        assert!(changed, "removing stale snapshots must report changes");
        assert!(!root.join("chatbox-product-info").as_path().exists());
        assert!(!root.join("data-analysis").as_path().exists());
        let manifest = std::fs::read_to_string(root.join("manifest.json").as_path()).unwrap();
        assert!(!manifest.contains("chatbox-product-info"));
        assert!(!manifest.contains("data-analysis"));
    }

    #[tokio::test]
    async fn rejects_traversal_and_reserved_file_paths() {
        assert!(validate_remote_file_path("../escape.txt").is_none());
        assert!(validate_remote_file_path("/abs/path.txt").is_none());
        assert!(validate_remote_file_path("SKILL.md").is_none());
        assert!(validate_remote_file_path("skill.md").is_none());
        assert!(validate_remote_file_path("source.json").is_none());
        assert!(validate_remote_file_path("scripts/run.py").is_some());
    }

    #[tokio::test]
    async fn skips_files_with_invalid_paths_but_keeps_skill() {
        let (_tmp, home) = ody_home();
        let options = RemoteSyncOptions { products: vec![] };
        let fetcher = FakeFetcher::new(vec![("with-files", "hash-w", "body W")]).detail(
            "with-files",
            RemoteSkillDetail {
                name: "with-files".to_string(),
                description: None,
                body: "body W".to_string(),
                files: Some(vec![
                    super::RemoteSkillFile {
                        path: "../escape.txt".to_string(),
                        content: "nope".to_string(),
                    },
                    super::RemoteSkillFile {
                        path: "scripts/run.py".to_string(),
                        content: "print('ok')".to_string(),
                    },
                ]),
                version: None,
                hash: None,
                allowed_tools: None,
                metadata: None,
                license: None,
            },
        );

        sync_remote_builtin_skills(&home, &fetcher, &options, &[])
            .await
            .unwrap();

        let root = remote_builtin_cache_root_dir(&home);
        assert!(std::path::Path::new(&root.join("with-files").join("SKILL.md").as_path()).exists());
        assert!(std::path::Path::new(
            &root.join("with-files").join("scripts").join("run.py").as_path()
        )
        .exists());
        assert!(!root.join("with-files").join("..").join("escape.txt").as_path().exists());
    }

    #[tokio::test]
    async fn keeps_snapshot_when_fetch_fails() {
        let (_tmp, home) = ody_home();
        let options = RemoteSyncOptions { products: vec![] };
        let fetcher = FakeFetcher::new(vec![("stock-skill", "hash-s", "stock body")]);
        sync_remote_builtin_skills(&home, &fetcher, &options, &[])
            .await
            .unwrap();

        let mut offline = FakeFetcher::new(vec![]);
        offline.fail_manifest = true;
        let changed = sync_remote_builtin_skills(&home, &offline, &options, &[])
            .await
            .unwrap();

        assert!(!changed, "failed sync must report no changes");
        let root = remote_builtin_cache_root_dir(&home);
        assert!(skill_md(&root, "stock-skill").contains("stock body"));
    }

    #[tokio::test]
    async fn tags_products_and_gates_visibility() {
        use ody_core_skills::filter_skill_load_outcome_for_product;
        use ody_core_skills::loader::SkillRoot;
        use ody_core_skills::loader::load_skills_from_roots;
        use ody_exec_server::LocalFileSystem;
        use ody_protocol::protocol::Product;
        use ody_protocol::protocol::SkillScope;

        let (_tmp, home) = ody_home();
        let options = RemoteSyncOptions {
            products: vec![Product::OdyBox],
        };
        let fetcher = FakeFetcher::new(vec![("stock-skill", "hash-s", "stock body")]);
        sync_remote_builtin_skills(&home, &fetcher, &options, &[])
            .await
            .unwrap();

        let root = remote_builtin_cache_root_dir(&home);
        let metadata = std::fs::read_to_string(
            root.join("stock-skill")
                .join("agents")
                .join("odysseythink.yaml")
                .as_path(),
        )
        .unwrap();
        assert!(metadata.contains("odybox"), "metadata: {metadata}");

        let outcome = load_skills_from_roots(
            [SkillRoot {
                path: root,
                scope: SkillScope::System,
                file_system: Arc::new(LocalFileSystem::unsandboxed()),
                plugin_id: None,
                plugin_namespace: None,
                plugin_root: None,
            }],
            None,
        )
        .await;
        assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);

        let for_odybox =
            filter_skill_load_outcome_for_product(outcome.clone(), Some(Product::OdyBox));
        assert!(
            for_odybox
                .skills
                .iter()
                .any(|skill| skill.name == "stock-skill"),
            "synced skill must be visible under Product::OdyBox"
        );
        let for_ody = filter_skill_load_outcome_for_product(outcome, Some(Product::Ody));
        assert!(
            !for_ody.skills.iter().any(|skill| skill.name == "stock-skill"),
            "synced skill must be hidden under Product::Ody"
        );
    }
}

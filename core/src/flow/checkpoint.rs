//! Checkpoint persistence for Flow agent results (M2.1).
//!
//! Every completed `agent()` call is recorded under the SHA-256 of its fully
//! rendered prompt: same input ⇒ replayable output without re-spawning a
//! sub-agent. Entries live in one JSON file per (skill, plan fingerprint)
//! under `<ody_home>/flow-checkpoints/` so a failed or interrupted run
//! resumes on re-trigger while a changed plan starts clean. Writes are
//! eager (write-through) and published atomically (temp file + rename) so
//! an aborted run keeps every agent that finished before the abort.

use std::path::Path;
use std::path::PathBuf;

use ody_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

/// On-disk shape of one run's checkpoint file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CheckpointFile {
    pub(crate) skill_name: String,
    pub(crate) plan_fingerprint: String,
    /// prompt_key → raw agent output text.
    #[serde(default)]
    pub(crate) entries: serde_json::Map<String, serde_json::Value>,
}

/// Stable 16-hex-char identity of a plan's source text. Changing the plan
/// (even whitespace-free rewording of a template) starts a fresh run file.
pub(crate) fn plan_fingerprint(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    let mut fp = String::with_capacity(16);
    for byte in &digest[..8] {
        fp.push_str(&format!("{byte:02x}"));
    }
    fp
}

/// Cache key for one agent call: the SHA-256 of the fully rendered prompt.
/// Different args/items/plan templates render different prompts, so key
/// collisions across distinct runs cannot occur.
pub(crate) fn prompt_key(prompt: &str) -> String {
    let digest = Sha256::digest(prompt.as_bytes());
    let mut key = String::with_capacity(64);
    for byte in &digest {
        key.push_str(&format!("{byte:02x}"));
    }
    key
}

fn sanitize_skill_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn checkpoint_path(ody_home: &Path, skill_name: &str, fingerprint: &str) -> PathBuf {
    ody_home
        .join("flow-checkpoints")
        .join(format!("{}-{fingerprint}.json", sanitize_skill_name(skill_name)))
}

/// Write-through checkpoint store for one (skill, plan) run file.
#[derive(Debug)]
pub(crate) struct CheckpointStore {
    path: PathBuf,
    file: tokio::sync::Mutex<CheckpointFile>,
}

impl CheckpointStore {
    /// Open (or create) the run file. A file whose fingerprint does not
    /// match starts empty — stale plans never leak entries into a new run.
    pub(crate) async fn open(
        ody_home: &AbsolutePathBuf,
        skill_name: &str,
        fingerprint: &str,
    ) -> Self {
        let path = checkpoint_path(ody_home.as_ref(), skill_name, fingerprint);
        let loaded = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<CheckpointFile>(&text).ok())
            .filter(|file| file.plan_fingerprint == fingerprint);
        Self {
            path,
            file: tokio::sync::Mutex::new(loaded.unwrap_or_else(|| CheckpointFile {
                skill_name: skill_name.to_string(),
                plan_fingerprint: fingerprint.to_string(),
                entries: serde_json::Map::new(),
            })),
        }
    }

    pub(crate) async fn lookup(&self, prompt: &str) -> Option<String> {
        let file = self.file.lock().await;
        file.entries
            .get(&prompt_key(prompt))
            .and_then(|value| value.as_str().map(str::to_string))
    }

    /// Record one completed agent result and publish the file atomically.
    pub(crate) async fn record(&self, prompt: &str, output: &str) {
        let rendered = {
            let mut file = self.file.lock().await;
            file.entries
                .insert(prompt_key(prompt), serde_json::Value::String(output.to_string()));
            serde_json::to_string(&*file).expect("checkpoint file serializes")
        };
        let tmp = self.path.with_extension("json.tmp");
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&tmp, rendered).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    /// Remove the run file after a fully successful run (no resume needed).
    pub(crate) async fn discard(&self) {
        self.file.lock().await.entries.clear();
        let _ = std::fs::remove_file(&self.path);
    }
}

use std::path::Path;
use std::path::PathBuf;

use fs2::FileExt;
use ody_shell_command::RealFsChecker;
use ody_shell_command::detect_windows_bash;
use serde::Serialize;

/// Name of the lock file used to serialize writes to `config.toml`.
const CONFIG_LOCK_FILE: &str = ".config.toml.lock";

/// Result of resolving the Windows bash shell for the current session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShellConfigResult {
    /// Absolute path to the bash executable that will be used.
    pub shell: PathBuf,
    /// Whether the path was auto-detected and persisted to the user's config.
    pub auto_detected_and_persisted: bool,
}

/// Process-scoped advisory lock for the user `config.toml`.
///
/// Holds an exclusive fs2 lock on a dedicated lock file for the lifetime of the
/// value. Dropping the guard releases the lock.
pub struct ConfigFileLock {
    #[allow(dead_code)]
    lock_file: std::fs::File,
}

impl ConfigFileLock {
    /// Acquires an exclusive lock on the lock file inside `ody_home`.
    ///
    /// The lock file is created if it does not exist.
    pub fn new(ody_home: &Path) -> std::io::Result<Self> {
        let lock_path = ody_home.join(CONFIG_LOCK_FILE);
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock_file.lock_exclusive()?;
        Ok(Self { lock_file })
    }
}

impl Drop for ConfigFileLock {
    fn drop(&mut self) {
        // Best-effort release. fs2 automatically drops the OS lock when the
        // file is closed, but we unlock explicitly to avoid relying on that.
        let _ = self.lock_file.unlock();
    }
}

/// On Windows, when `current_shell` is unset, detect a native bash (Git
/// Bash/MSYS2/Cygwin/PATH), persist the absolute path to `config.toml`, and
/// return the detected path.
///
/// On non-Windows platforms, or when a shell is already configured, returns
/// `None` immediately without touching the filesystem.
///
/// The detection work is performed in `spawn_blocking` so the async runtime is
/// not blocked by filesystem checks or by running `bash --version`.
pub async fn resolve_windows_shell(
    ody_home: &Path,
    current_shell: Option<&str>,
) -> Option<ShellConfigResult> {
    if cfg!(not(windows)) {
        return None;
    }
    if current_shell.map(|s| !s.is_empty()).unwrap_or(false) {
        return None;
    }

    let ody_home = ody_home.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let fs = RealFsChecker;
        let detection = detect_windows_bash(&fs)?;
        let shell_path = detection.shell.shell_path;

        // Even if persistence fails, use the detected shell for the current
        // session. Only report `auto_detected_and_persisted = true` when the
        // write actually succeeds.
        let persisted = persist_shell_blocking(&ody_home, &shell_path).is_ok();
        Some(ShellConfigResult {
            shell: shell_path,
            auto_detected_and_persisted: persisted,
        })
    })
    .await
    .ok()
    .flatten()
}

/// Persists `shell` as the `shell` key in `ody_home/config.toml`.
///
/// Callers are expected to already hold a `ConfigFileLock` if they need to
/// serialize with other writers. This function does its own locking for
/// standalone use.
fn persist_shell_blocking(ody_home: &Path, shell: &Path) -> std::io::Result<()> {
    let _lock = ConfigFileLock::new(ody_home)?;
    let config_path = ody_home.join(crate::CONFIG_TOML_FILE);

    let contents = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };

    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    doc["shell"] = toml_edit::value(shell.to_string_lossy().to_string());

    std::fs::write(&config_path, doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_lock_can_be_acquired_and_released() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let lock = ConfigFileLock::new(tmp.path()).unwrap();
            drop(lock);
        }
        let lock = ConfigFileLock::new(tmp.path()).unwrap();
        drop(lock);
    }

    #[test]
    fn config_file_lock_blocks_concurrent_shell_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let first = ConfigFileLock::new(tmp.path()).unwrap();

        // Attempt to acquire a second lock in a separate thread without
        // blocking, so we can assert it fails while the first is held.
        let tmp_path = tmp.path().to_path_buf();
        let second = std::thread::spawn(move || {
            let lock_file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(tmp_path.join(CONFIG_LOCK_FILE))
                .unwrap();
            lock_file.try_lock_exclusive()
        });

        let second_result = second.join().unwrap();
        assert!(
            second_result.is_err(),
            "second lock acquisition should be blocked while first lock is held"
        );

        drop(first);
    }

    #[test]
    fn persist_shell_blocking_creates_config_and_sets_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let shell = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");

        persist_shell_blocking(tmp.path(), &shell).unwrap();

        let config_path = tmp.path().join(crate::CONFIG_TOML_FILE);
        let contents = std::fs::read_to_string(&config_path).unwrap();
        let doc = contents.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(
            doc["shell"].as_str(),
            Some(r"C:\Program Files\Git\bin\bash.exe")
        );
    }

    #[test]
    fn persist_shell_blocking_updates_existing_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(crate::CONFIG_TOML_FILE);
        std::fs::write(&config_path, "shell = \"old_shell.exe\"\n").unwrap();

        let shell = PathBuf::from(r"C:\msys64\usr\bin\bash.exe");
        persist_shell_blocking(tmp.path(), &shell).unwrap();

        let contents = std::fs::read_to_string(&config_path).unwrap();
        let doc = contents.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(
            doc["shell"].as_str(),
            Some(r"C:\msys64\usr\bin\bash.exe")
        );
    }

    #[tokio::test]
    async fn resolve_windows_shell_skips_when_shell_is_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_windows_shell(tmp.path(), Some("bash.exe")).await;
        assert!(result.is_none());
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn resolve_windows_shell_noop_on_non_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_windows_shell(tmp.path(), None).await;
        // On non-Windows targets detection returns None immediately.
        assert!(result.is_none());
    }
}

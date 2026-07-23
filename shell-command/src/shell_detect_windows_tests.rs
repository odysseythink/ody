//! Windows bash auto-detection tests using an injected [`FsChecker`].
//!
//! These tests run on every platform because they never execute a real
//! `bash --version`; the working-executable check is mocked through the
//! [`FsChecker::is_bash_executable`] hook.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use crate::FsChecker;
use crate::RealFsChecker;
use crate::WindowsBashDetection;
use crate::detect_windows_bash;

#[derive(Default)]
struct MockFsChecker {
    files: HashMap<PathBuf, bool>,
    path_env: String,
    working_bash_paths: Vec<PathBuf>,
}

impl FsChecker for MockFsChecker {
    fn is_file(&self, path: &Path) -> bool {
        self.files.get(path).copied().unwrap_or(false)
    }

    fn env_var(&self, _name: &str) -> Option<String> {
        None
    }

    fn path_env(&self) -> Option<String> {
        Some(self.path_env.clone())
    }

    fn is_bash_executable(&self, path: &Path) -> bool {
        self.working_bash_paths.iter().any(|p| p == path)
    }
}

#[test]
fn detect_windows_bash_prefers_fixed_paths() {
    let mut fs = MockFsChecker::default();
    fs.files.insert(
        PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe"),
        true,
    );
    fs.working_bash_paths
        .push(PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe"));

    let detection = detect_windows_bash(&fs).expect("bash should be detected");
    assert_eq!(
        detection.shell.shell_path,
        PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe")
    );
    assert!(detection.is_fixed_path);
}

#[test]
fn detect_windows_bash_skips_wsl_paths() {
    let mut fs = MockFsChecker::default();
    let wsl_path = PathBuf::from(r"C:\Users\Alice\AppData\Local\Microsoft\WindowsApps\bash.exe");
    fs.files.insert(wsl_path.clone(), true);
    fs.working_bash_paths.push(wsl_path);

    // Also provide a valid fixed-path bash so detection succeeds.
    fs.files
        .insert(PathBuf::from(r"C:\msys64\usr\bin\bash.exe"), true);
    fs.working_bash_paths
        .push(PathBuf::from(r"C:\msys64\usr\bin\bash.exe"));

    let detection = detect_windows_bash(&fs).expect("bash should be detected");
    assert_eq!(
        detection.shell.shell_path,
        PathBuf::from(r"C:\msys64\usr\bin\bash.exe")
    );
    assert!(detection.is_fixed_path);
}

#[test]
fn detect_windows_bash_falls_back_to_path() {
    let mut fs = MockFsChecker::default();
    fs.path_env = r"C:\CustomTools".to_string();
    let path_bash = PathBuf::from(r"C:\CustomTools\bash.exe");
    fs.files.insert(path_bash.clone(), true);
    fs.working_bash_paths.push(path_bash);

    let detection = detect_windows_bash(&fs).expect("bash should be detected");
    assert_eq!(
        detection.shell.shell_path,
        PathBuf::from(r"C:\CustomTools\bash.exe")
    );
    assert!(!detection.is_fixed_path);
}

#[test]
fn detect_windows_bash_returns_none_when_no_candidate_works() {
    let fs = MockFsChecker::default();
    assert!(detect_windows_bash(&fs).is_none());
}

#[test]
fn detect_windows_bash_ignores_non_executable_files() {
    let mut fs = MockFsChecker::default();
    fs.files.insert(
        PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe"),
        true,
    );
    // file exists but is not a working bash executable

    assert!(detect_windows_bash(&fs).is_none());
}

#[test]
fn real_fs_checker_implements_trait() {
    // Smoke test that the production implementation still satisfies the trait.
    let _fs: &dyn FsChecker = &RealFsChecker;
}

#[cfg(not(windows))]
#[test]
fn detect_windows_bash_returns_none_on_non_windows() {
    let fs = MockFsChecker::default();
    // On non-Windows targets the function returns early regardless of mocks.
    assert!(detect_windows_bash(&fs).is_none());
}

#[cfg(windows)]
#[test]
fn detect_windows_bash_fixed_path_ordering() {
    let mut fs = MockFsChecker::default();
    // Mark the first two fixed paths as present and working; expect the first.
    fs.files.insert(
        PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe"),
        true,
    );
    fs.files
        .insert(PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"), true);
    fs.working_bash_paths
        .push(PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe"));
    fs.working_bash_paths
        .push(PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"));

    let detection = detect_windows_bash(&fs).expect("bash should be detected");
    assert_eq!(
        detection.shell.shell_path,
        PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe")
    );
    assert!(detection.is_fixed_path);
}

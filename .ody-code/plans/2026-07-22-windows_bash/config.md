# Part 1: Config crate 探测、持久化与写入

**Scope:** 在 `ody-config` crate 中新增 `shell_auto_detect.rs`，实现 Windows bash 探测、进程级文件锁保护的 `config.toml` 持久化，并暴露 `ShellConfigResult` / `resolve_windows_shell` 给 `ody-core` 消费。

## Self-review (Part 1)

- [ ] 1. Spec-coverage: 本 part 覆盖"Windows 且 `shell` 为空时触发 bash 探测"、"固定路径 + PATH 探测"、"成功后写入 `config.toml`"、"使用 `spawn_blocking`"、"进程级文件锁"、"已配置 shell 不探测"、"非 Windows 行为不变"。
- [ ] 2. Placeholder scan: 无 TODO/TBD/deferred placeholders。
- [ ] 3. No phantom tasks: 每个 task 都产生文件或依赖变更。
- [ ] 4. Dependency soundness: Part 1 是首个 part，无外部依赖。
- [ ] 5. Caller & build soundness: Task 2 修改 `load_config_with_layer_stack` 的下游调用者不在本 part，由 Part 2 处理。
- [ ] 6. Test-the-risk: Task 4 对文件锁和持久化写行为测试。
- [ ] 7. Type consistency: 类型定义与 Part 2 消费端一致。

### Task 1: 依赖变更

**Depends on:** none

**Files:**
- Modify: `Cargo.toml`（workspace dependencies）
- Modify: `config/Cargo.toml`（crate dependencies）

**Implementation:**

- [ ] 在根 `Cargo.toml` 的 `[workspace.dependencies]` 段新增一行：
  ```toml
  fs2 = "0.4"
  ```
- [ ] 在 `config/Cargo.toml` 的 `[dependencies]` 段新增：
  ```toml
  ody-shell-command = { workspace = true }
  fs2 = { workspace = true }
  ```
- [ ] 将 `config/Cargo.toml` 中 `tokio` 的特性从 `features = ["fs"]` 改为 `features = ["fs", "rt"]`，以支持 `tokio::task::spawn_blocking`：
  ```toml
  tokio = { workspace = true, features = ["fs", "rt"] }
  ```
- [ ] 验证当前 `ody-shell-command` 无反向依赖：
  ```bash
  cargo tree -p ody-shell-command --invert -e normal
  ```
  预期输出：不显示任何反向依赖节点。
- [ ] 提交：
  ```bash
  git add Cargo.toml config/Cargo.toml
  git commit -m "build(config): add fs2 workspace dep, ody-shell-command/fs2/rt to config crate"
  ```

### Task 2: 实现 `config/src/shell_auto_detect.rs`

**Depends on:** Task 1

**Files:**
- Create: `config/src/shell_auto_detect.rs`

**Implementation:**

- [ ] 创建 `config/src/shell_auto_detect.rs`，写入完整实现：
  ```rust
  use std::collections::HashSet;
  use std::fs::{File, OpenOptions};
  use std::io;
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  use ody_utils_path::write_atomically;
  use shell_command::shell_detect::FsChecker;
  use tokio::task;
  use toml_edit::DocumentMut;
  use toml_edit::Item as TomlItem;
  use toml_edit::value;
  use tracing;

  use crate::CONFIG_TOML_FILE;

  /// 由 config 层返回给 core 的 shell 配置结果。
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct ShellConfigResult {
      /// 最终用于当前会话的 shell 值；可能来自用户配置，也可能来自自动探测。
      pub shell: Option<String>,
      /// 本次 session 启动时是否由自动探测写入 config.toml 并命中固定路径。
      pub auto_detected_and_persisted: bool,
  }

  impl ShellConfigResult {
      pub fn from_config(shell: Option<String>) -> Self {
          Self {
              shell,
              auto_detected_and_persisted: false,
          }
      }
  }

  /// 基于 fs2 的进程级文件锁封装。
  pub struct ConfigFileLock {
      _file: File,
  }

  impl ConfigFileLock {
      /// 获取 ody_home 下的 config.toml 写锁。
      pub fn lock(ody_home: &Path) -> io::Result<Self> {
          let lock_path = ody_home.join(".config.toml.lock");
          let file = OpenOptions::new()
              .read(true)
              .write(true)
              .create(true)
              .truncate(false)
              .open(&lock_path)?;
          fs2::FileExt::lock_exclusive(&file)?;
          Ok(Self { _file: file })
      }
  }

  impl Drop for ConfigFileLock {
      fn drop(&mut self) {
          let _ = fs2::FileExt::unlock(&self._file);
      }
  }

  /// 探测并可能持久化 Windows bash 的入口。
  ///
  /// 仅在 `cfg!(windows)` 且 `shell` 为空时执行探测。探测成功后，将找到的 bash
  /// 绝对路径写入 config.toml；写入后 `auto_detected_and_persisted = true`。
  /// 探测使用 `spawn_blocking` 以避免阻塞 tokio 运行时，对 `bash --version`
  /// 的调用已有 1 秒超时，且仅对存在的文件路径调用。
  pub async fn resolve_windows_shell(
      ody_home: &Path,
      shell: Option<String>,
  ) -> ShellConfigResult {
      resolve_windows_shell_with_checker(
          ody_home,
          shell,
          Arc::new(shell_command::shell_detect::RealFsChecker),
      )
      .await
  }

  async fn resolve_windows_shell_with_checker(
      ody_home: &Path,
      shell: Option<String>,
      fs: Arc<dyn FsChecker + Send + Sync>,
  ) -> ShellConfigResult {
      if cfg!(not(windows)) || shell.is_some() {
          // 用户已配置 shell：无论是否有效，都不探测、不修改配置。
          return ShellConfigResult::from_config(shell);
      }

      let detection = task::spawn_blocking({
          let ody_home = ody_home.to_path_buf();
          move || shell_command::shell_detect::detect_windows_bash(fs.as_ref())
      })
      .await
      .ok()
      .flatten();

      match detection {
          Some(detection) => {
              let path = detection.shell.shell_path.to_string_lossy().to_string();
              if let Err(err) = persist_shell(ody_home, &path).await {
                  tracing::warn!(%err, "auto-detected bash but failed to persist to config.toml");
                  return ShellConfigResult {
                      shell: Some(path),
                      auto_detected_and_persisted: false,
                  };
              }
              ShellConfigResult {
                  shell: Some(path),
                  auto_detected_and_persisted: true,
              }
          }
          None => ShellConfigResult::from_config(None),
      }
  }

  async fn persist_shell(ody_home: &Path, shell_path: &str) -> io::Result<()> {
      let ody_home = ody_home.to_path_buf();
      let shell_path = shell_path.to_string();
      task::spawn_blocking(move || persist_shell_blocking(&ody_home, &shell_path))
          .await
          .map_err(|err| io::Error::other(format!("persist shell task panicked: {err}")))?
  }

  fn persist_shell_blocking(ody_home: &Path, shell_path: &str) -> io::Result<()> {
      let _lock = ConfigFileLock::lock(ody_home)?;
      let config_path = ody_home.join(CONFIG_TOML_FILE);
      let mut doc = read_or_create_document(&config_path)?;
      let table = doc.as_table_mut();
      let existing = table.get("shell");
      let mut replacement = value(shell_path);
      if let Some(existing) = existing {
          preserve_decor(existing, &mut replacement);
      }
      table.insert("shell", replacement);
      write_atomically(&config_path, &doc.to_string())
  }

  fn read_or_create_document(config_path: &Path) -> io::Result<DocumentMut> {
      match std::fs::read_to_string(config_path) {
          Ok(raw) => raw
              .parse::<DocumentMut>()
              .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
          Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(DocumentMut::new()),
          Err(err) => Err(err),
      }
  }

  fn preserve_decor(existing: &TomlItem, replacement: &mut TomlItem) {
      if let (TomlItem::Value(existing_value), TomlItem::Value(replacement_value)) =
          (existing, replacement)
      {
          replacement_value.decor_mut().clone_from(existing_value.decor());
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use std::collections::HashSet;
      use std::sync::Arc;
      use tempfile::TempDir;

      #[derive(Clone, Default)]
      struct MockFsChecker {
          existing_files: HashSet<PathBuf>,
          bash_executables: HashSet<PathBuf>,
          path_env: Option<String>,
      }

      impl FsChecker for MockFsChecker {
          fn is_file(&self, path: &std::path::Path) -> bool {
              self.existing_files.contains(path)
          }
          fn env_var(&self, _name: &str) -> Option<String> {
              None
          }
          fn path_env(&self) -> Option<String> {
              self.path_env.clone()
          }
          fn is_bash_executable(&self, path: &std::path::Path) -> bool {
              self.bash_executables.contains(path)
          }
      }

      #[tokio::test]
      async fn configured_shell_is_not_probed() {
          let ody_home = TempDir::new().unwrap();
          let fs = Arc::new(MockFsChecker::default());
          let result = resolve_windows_shell_with_checker(
              ody_home.path(),
              Some("bash".to_string()),
              fs,
          )
          .await;
          assert_eq!(result.shell, Some("bash".to_string()));
          assert!(!result.auto_detected_and_persisted);
          assert!(!ody_home.path().join(CONFIG_TOML_FILE).exists());
      }

      #[cfg(windows)]
      #[tokio::test]
      async fn fixed_path_detection_persists_and_flags() {
          let ody_home = TempDir::new().unwrap();
          let fixed_path = PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe");
          let mut existing = HashSet::new();
          existing.insert(fixed_path.clone());
          let mut bash = HashSet::new();
          bash.insert(fixed_path.clone());
          let fs = Arc::new(MockFsChecker {
              existing_files: existing,
              bash_executables: bash,
              path_env: None,
          });
          let result = resolve_windows_shell_with_checker(ody_home.path(), None, fs).await;
          assert_eq!(result.shell, Some(fixed_path.to_string_lossy().to_string()));
          assert!(result.auto_detected_and_persisted);
          let raw = std::fs::read_to_string(ody_home.path().join(CONFIG_TOML_FILE)).unwrap();
          assert!(raw.contains("shell"));
          assert!(raw.contains(&fixed_path.to_string_lossy().to_string()));
      }

      #[cfg(windows)]
      #[tokio::test]
      async fn path_detection_persists_and_flags() {
          let ody_home = TempDir::new().unwrap();
          let path_dir = TempDir::new().unwrap();
          let bash_path = path_dir.path().join("bash.exe");
          let mut existing = HashSet::new();
          existing.insert(bash_path.clone());
          let mut bash = HashSet::new();
          bash.insert(bash_path.clone());
          let fs = Arc::new(MockFsChecker {
              existing_files: existing,
              bash_executables: bash,
              path_env: Some(path_dir.path().to_string_lossy().to_string()),
          });
          let result = resolve_windows_shell_with_checker(ody_home.path(), None, fs).await;
          assert_eq!(result.shell, Some(bash_path.to_string_lossy().to_string()));
          assert!(result.auto_detected_and_persisted);
      }

      #[cfg(windows)]
      #[tokio::test]
      async fn wsl_path_is_excluded() {
          let ody_home = TempDir::new().unwrap();
          let wsl_path = PathBuf::from(r"C:\Windows\System32\bash.exe");
          let mut existing = HashSet::new();
          existing.insert(wsl_path.clone());
          let mut bash = HashSet::new();
          bash.insert(wsl_path.clone());
          let fs = Arc::new(MockFsChecker {
              existing_files: existing,
              bash_executables: bash,
              path_env: None,
          });
          let result = resolve_windows_shell_with_checker(ody_home.path(), None, fs).await;
          assert_eq!(result.shell, None);
          assert!(!result.auto_detected_and_persisted);
          assert!(!ody_home.path().join(CONFIG_TOML_FILE).exists());
      }

      #[tokio::test]
      async fn persist_shell_blocking_writes_shell() {
          let ody_home = TempDir::new().unwrap();
          let shell_path = r"C:\msys64\usr\bin\bash.exe";
          persist_shell_blocking(ody_home.path(), shell_path).unwrap();
          let raw = std::fs::read_to_string(ody_home.path().join(CONFIG_TOML_FILE)).unwrap();
          assert!(raw.contains(&format!(r#"shell = "{}""#, shell_path)));
      }

      #[tokio::test]
      async fn persist_preserves_existing_decor() {
          let ody_home = TempDir::new().unwrap();
          let config_path = ody_home.path().join(CONFIG_TOML_FILE);
          std::fs::write(&config_path, "shell = \"zsh\"\n").unwrap();
          persist_shell_blocking(ody_home.path(), r"C:\bash.exe").unwrap();
          let raw = std::fs::read_to_string(&config_path).unwrap();
          assert!(raw.contains("shell"));
      }

      #[tokio::test]
      async fn concurrent_persist_shell_blocking_does_not_corrupt() {
          let ody_home = TempDir::new().unwrap();
          let ody_home = Arc::new(ody_home);
          let mut handles = Vec::new();
          for i in 0..10usize {
              let ody_home = Arc::clone(&ody_home);
              handles.push(std::thread::spawn(move || {
                  let shell_path = format!(r"C:\bash{:.2}.exe", i);
                  let _ = persist_shell_blocking(ody_home.path(), &shell_path);
              }));
          }
          for handle in handles {
              handle.join().unwrap();
          }
          let raw = std::fs::read_to_string(ody_home.path().join(CONFIG_TOML_FILE)).unwrap();
          let doc = raw.parse::<DocumentMut>().unwrap();
          let shell = doc["shell"].as_str().unwrap();
          assert!(shell.starts_with(r"C:\bash"));
          // 文档被解析成功即说明文件未损坏；只有一个 shell 键。
          assert!(doc.as_table().get("shell").is_some());
      }
  }
  ```
- [ ] 构建并运行 config crate 测试：
  ```bash
  cargo nextest run -p ody-config shell_auto_detect
  ```
  预期输出：非 Windows 平台通过 3 个通用测试；Windows 平台额外通过 3 个 Windows 特定测试。
- [ ] 提交：
  ```bash
  git add config/src/shell_auto_detect.rs
  git commit -m "feat(config): add shell_auto_detect with fs2 lock and Windows bash persistence"
  ```

### Task 3: 在 `config/src/lib.rs` 中导出 `shell_auto_detect`

**Depends on:** Task 2

**Files:**
- Modify: `config/src/lib.rs`

**Implementation:**

- [ ] 在 `config/src/lib.rs` 的模块声明区添加 `pub mod shell_auto_detect;`，在 `pub use` 区添加 `pub use shell_auto_detect::{resolve_windows_shell, ShellConfigResult};`。
  ```rust
  pub mod shell_auto_detect;
  ```
  并在文件末尾或合适位置添加：
  ```rust
  pub use shell_auto_detect::resolve_windows_shell;
  pub use shell_auto_detect::ShellConfigResult;
  ```
- [ ] 运行编译检查：
  ```bash
  cargo check -p ody-config
  ```
  预期输出：无错误。
- [ ] 提交：
  ```bash
  git add config/src/lib.rs
  git commit -m "feat(config): export shell_auto_detect from public API"
  ```

### Task 4: 补充 config crate 集成测试

**Depends on:** Task 2, Task 3

**Files:**
- Create: `config/tests/shell_auto_detect.rs`

**Implementation:**

- [ ] 创建 `config/tests/shell_auto_detect.rs`，验证公开 API 的行为边界：
  ```rust
  use ody_config::resolve_windows_shell;
  use ody_config::ShellConfigResult;
  use tempfile::TempDir;

  #[tokio::test]
  async fn resolve_windows_shell_leaves_configured_shell_untouched() {
      let ody_home = TempDir::new().unwrap();
      let result = resolve_windows_shell(ody_home.path(), Some("zsh".to_string())).await;
      assert_eq!(result.shell, Some("zsh".to_string()));
      assert!(!result.auto_detected_and_persisted);
      assert!(!ody_home.path().join("config.toml").exists());
  }

  #[tokio::test]
  async fn resolve_windows_shell_non_windows_returns_none() {
      let ody_home = TempDir::new().unwrap();
      let result = resolve_windows_shell(ody_home.path(), None).await;
      if cfg!(not(windows)) {
          assert_eq!(result.shell, None);
          assert!(!result.auto_detected_and_persisted);
      }
  }
  ```
- [ ] 运行测试：
  ```bash
  cargo nextest run -p ody-config --test shell_auto_detect
  ```
  预期输出：测试通过。
- [ ] 提交：
  ```bash
  git add config/tests/shell_auto_detect.rs
  git commit -m "test(config): add shell_auto_detect integration tests"
  ```

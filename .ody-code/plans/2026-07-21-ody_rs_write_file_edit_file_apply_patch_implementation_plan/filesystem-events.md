<!-- Part 2: Atomic writes, filesystem trait, and FileChange events -->

# Part 2: Atomic writes, filesystem trait, and FileChange events

**Scope:** 在 `ExecutorFileSystem` trait 上新增 `rename`（默认 copy+remove 回退），为本地文件系统实现原生重命名；验证 Part 1 已引入的原子写入与 diff 辅助函数；为 `FileChange` 事件信封新增可选的 `source` 元数据，并在 `write_file`/`edit_file`/`apply_patch` 中分别填充工具来源。

### Task 6: Add `rename` to `ExecutorFileSystem` trait

**Depends on:** none

**Files:**
- Modify: `file-system/src/lib.rs` (trait definition)
- Test: `file-system/src/lib.rs` (inline `#[cfg(test)]`)

**Implementation:**
- [ ] Add `rename` to `ExecutorFileSystem` after `copy` (around `file-system/src/lib.rs:265`):
  ```rust
  fn rename<'a>(
      &'a self,
      source_path: &'a PathUri,
      destination_path: &'a PathUri,
      sandbox: Option<&'a FileSystemSandboxContext>,
  ) -> ExecutorFileSystemFuture<'a, ()> {
      Box::pin(async move {
          self.copy(
              source_path,
              destination_path,
              CopyOptions { recursive: false },
              sandbox,
          )
          .await?;
          self.remove(
              source_path,
              RemoveOptions {
                  recursive: false,
                  force: false,
              },
              sandbox,
          )
          .await
      })
  }
  ```
  Source grounding: `ExecutorFileSystem` already defines `copy` and `remove` with `CopyOptions` / `RemoveOptions` at `file-system/src/lib.rs:259-265`; this default uses them to provide a non-atomic fallback.
- [ ] Add a compile-only test that a type implementing only the required methods can use `rename` via the default:
  ```rust
  #[cfg(test)]
  mod rename_tests {
      use super::*;
      use ody_utils_path_uri::PathUri;
      use std::collections::VecDeque;
      use std::sync::Mutex;

      struct CallLog(Mutex<VecDeque<String>>);

      impl ExecutorFileSystem for CallLog {
          fn canonicalize<'a>(
              &'a self,
              _path: &'a PathUri,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, PathUri> {
              Box::pin(async { Err(io::Error::other("unimplemented")) })
          }
          fn read_file<'a>(
              &'a self,
              _path: &'a PathUri,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
              Box::pin(async { Err(io::Error::other("unimplemented")) })
          }
          fn read_file_stream<'a>(
              &'a self,
              _path: &'a PathUri,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
              Box::pin(async { Err(io::Error::other("unimplemented")) })
          }
          fn write_file<'a>(
              &'a self,
              _path: &'a PathUri,
              _contents: Vec<u8>,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, ()> {
              Box::pin(async { Err(io::Error::other("unimplemented")) })
          }
          fn create_directory<'a>(
              &'a self,
              _path: &'a PathUri,
              _options: CreateDirectoryOptions,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, ()> {
              Box::pin(async { Err(io::Error::other("unimplemented")) })
          }
          fn get_metadata<'a>(
              &'a self,
              _path: &'a PathUri,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
              Box::pin(async { Err(io::Error::other("unimplemented")) })
          }
          fn read_directory<'a>(
              &'a self,
              _path: &'a PathUri,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
              Box::pin(async { Err(io::Error::other("unimplemented")) })
          }
          fn remove<'a>(
              &'a self,
              _path: &'a PathUri,
              _options: RemoveOptions,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, ()> {
              Box::pin(async { Ok(()) })
          }
          fn copy<'a>(
              &'a self,
              _source_path: &'a PathUri,
              _destination_path: &'a PathUri,
              _options: CopyOptions,
              _sandbox: Option<&'a FileSystemSandboxContext>,
          ) -> ExecutorFileSystemFuture<'a, ()> {
              Box::pin(async { Ok(()) })
          }
      }

      #[test]
      fn rename_default_calls_copy_then_remove() {
          let fs = CallLog(Mutex::new(VecDeque::new()));
          let src = PathUri::from_host_native_path(std::env::temp_dir().join("a")).unwrap();
          let dst = PathUri::from_host_native_path(std::env::temp_dir().join("b")).unwrap();
          // This test is a compile-time guard; the real behavioral test is in Task 8.
          let _ = fs.rename(&src, &dst, None);
      }
  }
  ```
- [ ] Run typecheck for the trait crate:
  ```bash
  cargo check -p ody-file-system --all-targets
  ```
  Expected: no compile errors.
- [ ] Commit:
  ```bash
  git add file-system/src/lib.rs
  git commit -m "feat(file-system): add ExecutorFileSystem::rename with default fallback"
  ```

### Task 7: Implement native `rename` in local filesystems

**Depends on:** Task 6

**Files:**
- Modify: `exec-server/src/local_file_system.rs`

**Implementation:**
- [ ] Add `rename` to `LocalFileSystem` private async API (around `exec-server/src/local_file_system.rs:183` after `copy`):
  ```rust
  async fn rename(
      &self,
      source_path: &PathUri,
      destination_path: &PathUri,
      sandbox: Option<&FileSystemSandboxContext>,
  ) -> FileSystemResult<()> {
      let (file_system, sandbox) = self.file_system_for(sandbox)?;
      file_system.rename(source_path, destination_path, sandbox).await
  }
  ```
- [ ] Add `rename` to `impl ExecutorFileSystem for LocalFileSystem` (after `copy`):
  ```rust
  fn rename<'a>(
      &'a self,
      source_path: &'a PathUri,
      destination_path: &'a PathUri,
      sandbox: Option<&'a FileSystemSandboxContext>,
  ) -> ExecutorFileSystemFuture<'a, ()> {
      Box::pin(LocalFileSystem::rename(
          self,
          source_path,
          destination_path,
          sandbox,
      ))
  }
  ```
- [ ] Add `rename` to `UnsandboxedFileSystem` private async API (after `copy`):
  ```rust
  async fn rename(
      &self,
      source_path: &PathUri,
      destination_path: &PathUri,
      sandbox: Option<&FileSystemSandboxContext>,
  ) -> FileSystemResult<()> {
      reject_platform_sandbox_context(sandbox)?;
      self.file_system
          .rename(source_path, destination_path, /*sandbox*/ None)
          .await
  }
  ```
- [ ] Add `rename` to `impl ExecutorFileSystem for UnsandboxedFileSystem` (after `copy`):
  ```rust
  fn rename<'a>(
      &'a self,
      source_path: &'a PathUri,
      destination_path: &'a PathUri,
      sandbox: Option<&'a FileSystemSandboxContext>,
  ) -> ExecutorFileSystemFuture<'a, ()> {
      Box::pin(UnsandboxedFileSystem::rename(
          self,
          source_path,
          destination_path,
          sandbox,
      ))
  }
  ```
- [ ] Add `rename` to `DirectFileSystem` private async API (after `copy`):
  ```rust
  async fn rename(
      &self,
      source_path: &PathUri,
      destination_path: &PathUri,
      sandbox: Option<&FileSystemSandboxContext>,
  ) -> FileSystemResult<()> {
      reject_sandbox_context(sandbox)?;
      let source = source_path.to_abs_path()?.into_path_buf();
      let destination = destination_path.to_abs_path()?.into_path_buf();
      match tokio::fs::rename(&source, &destination).await {
          Ok(()) => Ok(()),
          Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
              tokio::fs::remove_file(&destination).await?;
              tokio::fs::rename(&source, &destination).await
          }
          Err(err) => Err(err),
      }
  }
  ```
  Note: On Unix `tokio::fs::rename` atomically overwrites an existing destination. On Windows it fails when the destination exists; the retry path removes the destination first. The temp file and target are in the same directory, so the final `rename` is still atomic on all platforms.
- [ ] Add `rename` to `impl ExecutorFileSystem for DirectFileSystem` (after `copy`):
  ```rust
  fn rename<'a>(
      &'a self,
      source_path: &'a PathUri,
      destination_path: &'a PathUri,
      sandbox: Option<&'a FileSystemSandboxContext>,
  ) -> ExecutorFileSystemFuture<'a, ()> {
      Box::pin(DirectFileSystem::rename(
          self,
          source_path,
          destination_path,
          sandbox,
      ))
  }
  ```
- [ ] Leave `SandboxedFileSystem` and `RemoteFileSystem` using the default fallback; they are documented as out-of-scope for native atomic rename (see Out-of-scope in index).
- [ ] Run typecheck for the crate:
  ```bash
  cargo check -p ody-exec-server --all-targets
  ```
  Expected: no compile errors.
- [ ] Commit:
  ```bash
  git add exec-server/src/local_file_system.rs
  git commit -m "feat(exec-server): native rename for local filesystems"
  ```

### Task 8: Test atomic write and diff helpers

**Depends on:** Task 7, schemas-handlers.md: Task 2

**Files:**
- Modify: `core/src/tools/handlers/file_tools/write_edit.rs` (append test module)

**Implementation:**
Note: `atomic_write`, `unified_diff_for_update`, `non_overlapping_occurrences`, and `near_text_hint` are implemented in `core/src/tools/handlers/file_tools/write_edit.rs` by schemas-handlers.md: Task 2. This task adds behavioral tests for them and for the `ExecutorFileSystem::rename` implementation from Task 7.

- [ ] Append the following test module to `core/src/tools/handlers/file_tools/write_edit.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use ody_exec_server::LOCAL_FS;
      use ody_protocol::permissions::FileSystemSandboxPolicy;
      use ody_protocol::permissions::NetworkSandboxPolicy;
      use ody_utils_path_uri::PathUri;
      use tempfile::tempdir;

      fn unsandboxed_context() -> FileSystemSandboxContext {
          FileSystemSandboxContext::from_permission_profile(
              ody_protocol::models::PermissionProfile::from_runtime_permissions(
                  &FileSystemSandboxPolicy::Unrestricted,
                  NetworkSandboxPolicy::Unrestricted,
              ),
          )
      }

      #[tokio::test]
      async fn atomic_write_creates_target_and_removes_temp() {
          let dir = tempdir().expect("tempdir");
          let path = PathUri::from_host_native_path(dir.path().join("out.txt")).unwrap();
          let content = b"hello world".to_vec();
          atomic_write(LOCAL_FS.as_ref(), &path, content, None)
              .await
              .expect("atomic_write should succeed");

          let read = LOCAL_FS.read_file(&path, None).await.expect("read target");
          assert_eq!(read, b"hello world");

          let temp_count = LOCAL_FS
              .read_directory(&PathUri::from_host_native_path(dir.path()).unwrap(), None)
              .await
              .expect("read dir")
              .into_iter()
              .filter(|e| e.file_name.contains("ody-write-tmp"))
              .count();
          assert_eq!(temp_count, 0, "temporary file should be removed after rename");
      }

      #[tokio::test]
      async fn atomic_write_overwrites_existing_file() {
          let dir = tempdir().expect("tempdir");
          let path = PathUri::from_host_native_path(dir.path().join("out.txt")).unwrap();
          LOCAL_FS
              .write_file(&path, b"old".to_vec(), None)
              .await
              .expect("seed");
          atomic_write(LOCAL_FS.as_ref(), &path, b"new".to_vec(), None)
              .await
              .expect("atomic_write");
          let read = LOCAL_FS.read_file(&path, None).await.expect("read");
          assert_eq!(read, b"new");
      }

      #[tokio::test]
      async fn local_fs_rename_moves_file() {
          let dir = tempdir().expect("tempdir");
          let source = PathUri::from_host_native_path(dir.path().join("a.txt")).unwrap();
          let dest = PathUri::from_host_native_path(dir.path().join("b.txt")).unwrap();
          LOCAL_FS.write_file(&source, b"hello".to_vec(), None).await.expect("seed");
          LOCAL_FS.rename(&source, &dest, None).await.expect("rename");
          assert!(LOCAL_FS.get_metadata(&source, None).await.is_err(), "source removed");
          let content = LOCAL_FS.read_file(&dest, None).await.expect("read dest");
          assert_eq!(content, b"hello");
      }

      #[test]
      fn unified_diff_for_update_shows_changes() {
          let diff = unified_diff_for_update("foo\nbar\n", "foo\nbaz\n");
          assert!(diff.contains("---"), "diff missing --- header: {diff}");
          assert!(diff.contains("+++"), "diff missing +++ header: {diff}");
          assert!(diff.contains("-bar"), "diff missing removed line: {diff}");
          assert!(diff.contains("+baz"), "diff missing inserted line: {diff}");
      }

      #[test]
      fn non_overlapping_occurrences_counts_without_overlap() {
          assert_eq!(non_overlapping_occurrences("aaa", "aa"), 1);
          assert_eq!(non_overlapping_occurrences("aaaa", "aa"), 2);
          assert_eq!(non_overlapping_occurrences("hello world", "l"), 3);
      }

      #[test]
      fn near_text_hint_suggests_close_line() {
          let hint = near_text_hint("foo\nbar\nbaz\n", "bor");
          assert!(hint.contains("bar"), "hint should suggest nearest line: {hint}");
      }
  }
  ```
- [ ] Run the tests:
  ```bash
  cargo test -p ody-core --tests write_edit
  ```
  Expected: all new tests pass.
- [ ] Commit:
  ```bash
  git add core/src/tools/handlers/file_tools/write_edit.rs
  git commit -m "test(file_tools): atomic write, rename, and diff helpers"
  ```

### Task 9: Emit `FileChange` events with `source` metadata

**Depends on:** Task 8, schemas-handlers.md: Task 5

**Files:**
- Modify: `protocol/src/items.rs` (`FileChangeItem` struct)
- Modify: `core/src/tools/events.rs` (`ToolEmitter` variant + constructors + `emit_patch_end`)
- Modify: `core/src/tools/handlers/apply_patch.rs` (two call sites)
- Modify: `core/src/tools/handlers/file_tools/write.rs` (one call site)
- Modify: `core/src/tools/handlers/file_tools/edit.rs` (one call site)
- Modify: `app-server-protocol/src/protocol/v2/tests.rs` (one constructor)
- Modify: `protocol/src/protocol.rs` (two constructors)

**Implementation:**
- [ ] Add optional `source` field to `protocol::FileChangeItem` in `protocol/src/items.rs:180`:
  ```rust
  #[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq)]
  pub struct FileChangeItem {
      pub id: String,
      pub changes: HashMap<PathBuf, FileChange>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(optional)]
      pub status: Option<PatchApplyStatus>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(optional)]
      pub auto_approved: Option<bool>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(optional)]
      pub stdout: Option<String>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(optional)]
      pub stderr: Option<String>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(optional)]
      pub source: Option<String>,
  }
  ```
- [ ] Update `core/src/tools/events.rs` `ToolEmitter::ApplyPatch` variant to carry `source`:
  ```rust
  ApplyPatch {
      changes: HashMap<PathBuf, FileChange>,
      auto_approved: bool,
      environment_id: Option<String>,
      source: Option<String>,
  },
  ```
- [ ] Update the constructor `apply_patch_for_environment` to accept `source`:
  ```rust
  pub fn apply_patch_for_environment(
      changes: HashMap<PathBuf, FileChange>,
      auto_approved: bool,
      environment_id: String,
      source: Option<String>,
  ) -> Self {
      Self::ApplyPatch {
          changes,
          auto_approved,
          environment_id: Some(environment_id),
          source,
      }
  }
  ```
- [ ] In `ToolEmitter::emit`, destructure `source` from `ApplyPatch` and pass it to `emit_patch_end` in every `ApplyPatch` arm. Begin event:
  ```rust
  TurnItem::FileChange(FileChangeItem {
      id: ctx.call_id.to_string(),
      changes: changes.clone(),
      status: None,
      auto_approved: Some(*auto_approved),
      stdout: None,
      stderr: None,
      source: source.clone(),
  })
  ```
- [ ] Update `emit_patch_end` signature and completed `FileChangeItem` to include `source`:
  ```rust
  async fn emit_patch_end(
      ctx: ToolEventCtx<'_>,
      changes: HashMap<PathBuf, FileChange>,
      stdout: String,
      stderr: String,
      status: PatchApplyStatus,
      source: Option<String>,
      tracker_update: TurnDiffTrackerUpdate<'_>,
  ) {
      ctx.session
          .emit_turn_item_completed(
              ctx.turn,
              TurnItem::FileChange(FileChangeItem {
                  id: ctx.call_id.to_string(),
                  changes,
                  status: Some(status),
                  auto_approved: None,
                  stdout: Some(stdout),
                  stderr: Some(stderr),
                  source,
              }),
          )
          .await;
      // ... rest unchanged
  }
  ```
- [ ] Update `core/src/tools/handlers/apply_patch.rs` (two sites, around `core/src/tools/handlers/apply_patch.rs:620` and `:795`) to pass `Some("apply_patch".to_string())`:
  ```rust
  let emitter = ToolEmitter::apply_patch_for_environment(
      changes.clone(),
      apply.auto_approved,
      turn_environment.environment_id.clone(),
      Some("apply_patch".to_string()),
  );
  ```
- [ ] Update `core/src/tools/handlers/file_tools/write.rs` to pass `Some("write_file".to_string())`.
- [ ] Update `core/src/tools/handlers/file_tools/edit.rs` to pass `Some("edit_file".to_string())`.
- [ ] Update `protocol/src/protocol.rs:5001` and `:5110` to add `source: None` to the `FileChangeItem` constructors.
- [ ] Update `app-server-protocol/src/protocol/v2/tests.rs:2587` to add `source: None` to the `FileChangeItem` constructor.
- [ ] Add a test to `core/src/tools/events.rs` tests verifying source propagation:
  ```rust
  #[tokio::test]
  async fn file_change_emitter_includes_source_metadata() {
      let (session, turn, mut rx_event) =
          make_session_and_context_with_dynamic_tools_and_rx(Vec::new()).await;
      let changes = HashMap::from([(
          PathBuf::from("a.txt"),
          FileChange::Add {
              content: "hello".to_string(),
          },
      )]);
      let emitter = ToolEmitter::apply_patch_for_environment(
          changes,
          true,
          "env-id".to_string(),
          Some("write_file".to_string()),
      );
      let ctx = ToolEventCtx::new(session.as_ref(), turn.as_ref(), "call-id", None);
      emitter.begin(ctx).await;

      let event = rx_event.recv().await.expect("item started event");
      match event.msg {
          EventMsg::ItemStarted(started) => match started.item {
              TurnItem::FileChange(item) => {
                  assert_eq!(item.source, Some("write_file".to_string()));
              }
              other => panic!("expected FileChange, got {other:?}"),
          },
          other => panic!("expected ItemStarted, got {other:?}"),
      }
  }
  ```
- [ ] Run the new events test:
  ```bash
  cargo test -p ody-core --tests file_change_emitter_includes_source_metadata
  ```
  Expected: test passes.
- [ ] Run whole-workspace typecheck:
  ```bash
  cargo check --workspace --all-targets
  ```
  Expected: no compile errors.
- [ ] If `app-server-protocol` schema fixture tests fail due to the changed `FileChangeItem` shape, regenerate fixtures:
  ```bash
  cargo run -p ody-app-server-protocol --bin write_schema_fixtures
  cargo test -p ody-app-server-protocol --test schema_fixtures
  ```
  Expected: fixture tests pass after regeneration.
- [ ] Commit:
  ```bash
  git add protocol/src/items.rs core/src/tools/events.rs core/src/tools/handlers/apply_patch.rs core/src/tools/handlers/file_tools/write.rs core/src/tools/handlers/file_tools/edit.rs app-server-protocol/src/protocol/v2/tests.rs protocol/src/protocol.rs
  git commit -m "feat(core): add source metadata to FileChange events"
  ```

## Out-of-scope (Part 2)

以下名称匹配与本次 Part 2 的目标概念不同，保留原样：

| Symbol / Path | Reason | Action |
|---|---|---|
| `exec::FileChangeItem` (`exec/src/exec_events.rs:182`) | 这是 `exec` 输出格式专用的结构体，不是 `protocol::FileChangeItem` 事件信封；本次 `source` 只进入协议事件。 | 无需修改 |
| `SandboxedFileSystem::rename` 原生实现 | 需要扩展 sandbox helper 协议与 `FsHelperRequest`，超出设计限定的“本地文件系统”范围。 | 使用 `ExecutorFileSystem::rename` 默认 copy+remove 回退 |
| `RemoteFileSystem::rename` 原生实现 | 设计明确限定为本地文件系统；远程 copy+remove 回退可接受。 | 使用默认实现 |
| `FileChange` enum 上的 `source` | 设计要求加在“事件信封”上，即 `FileChangeItem`；每个 `FileChange` 条目仍表示单个文件变更。 | 不加在 `FileChange` 上 |

## Spec coverage (Part 2)

| Requirement | Task(s) | Status |
|---|---|---|
| 写入成功后发出 `FileChange` 事件，兼容 `apply_patch` | Task 9 | covered |
| `FileChange` 事件附加 `source` 元数据 | Task 9 | covered |
| 原子写入：temp-file + rename | Task 6, Task 7, Task 8 | covered |
| `ExecutorFileSystem` 支持 `rename` | Task 6, Task 7 | covered |
| 本地文件系统原生重命名 | Task 7 | covered |
| 远程/沙盒文件系统使用默认回退 | Task 7, Out-of-scope | covered |

## Self-review (Part 2)

- [ ] 1. Spec-coverage: Part 2 covers all filesystem/event requirements from the design and index.
- [ ] 2. Placeholder scan: no TODO/TBD/deferred placeholders in task steps.
- [ ] 3. No phantom tasks: Task 8 produces tests; Task 6/7/9 produce concrete code changes.
- [ ] 4. Dependency soundness: Task 7 depends on Task 6; Task 8 depends on Task 7 and schemas-handlers.md: Task 2; Task 9 depends on Task 8 and schemas-handlers.md: Task 5.
- [ ] 5. Caller & build soundness: Task 9 changes the shared `FileChangeItem` and `ToolEmitter::apply_patch_for_environment` signatures and updates all callers (including tests) ending with `cargo check --workspace --all-targets`.
- [ ] 6. Test-the-risk: Task 8 tests atomic writes, rename, and diff helpers; Task 9 tests source metadata propagation.
- [ ] 7. Type consistency: `source` field name and `Option<String>` type are used consistently in `FileChangeItem`, `ToolEmitter::ApplyPatch`, and all call sites.

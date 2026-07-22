<!-- Part 1: Schemas, handlers, and registration -->

# Part 1: Schemas, handlers, and registration

**Scope:** 定义 `write_file` / `edit_file` 的 JSON Schema、实现两个 handler、完成模块导出与 turn plan 注册。

### Task 1: Add `write_file` / `edit_file` tool specs to `file_tools_spec.rs`

**Depends on:** none

**Files:**
- Modify: `core/src/tools/handlers/file_tools_spec.rs:22-25` (constants)
- Modify: `core/src/tools/handlers/file_tools_spec.rs:130-220` (new spec functions)
- Modify: `core/src/tools/handlers/file_tools_spec.rs:395-470` (tests)

**Implementation:**
- [ ] Add tool-name constants:
  ```rust
  pub const WRITE_FILE_TOOL_NAME: &str = "write_file";
  pub const EDIT_FILE_TOOL_NAME: &str = "edit_file";
  ```
- [ ] Add size constants (also used by handlers and tests):
  ```rust
  pub const MAX_WRITE_FILE_BYTES: usize = 10 * 1024 * 1024;
  pub const MAX_FILE_SIZE_FOR_DIFF: usize = 1024 * 1024;
  ```
- [ ] Implement `create_write_file_tool`:
  ```rust
  pub fn create_write_file_tool(options: FileToolOptions) -> ToolSpec {
      let mut properties = BTreeMap::from([
          (
              "path".to_string(),
              JsonSchema::string(Some(
                  "Path to the file to write. Relative paths are resolved against the working directory.".to_string(),
              )),
          ),
          (
              "content".to_string(),
              JsonSchema::string(Some(
                  format!("Text content to write. Maximum {} MiB per call.", MAX_WRITE_FILE_BYTES / 1024 / 1024),
              )),
          ),
          (
              "mode".to_string(),
              JsonSchema::string_enum(
                  vec![json!("overwrite"), json!("append")],
                  Some("Write mode. `overwrite` (default) replaces the file; `append` adds to the end.".to_string()),
              ),
          ),
      ]);
      environment_id_property(&mut properties, options);
      ToolSpec::Function(ResponsesApiTool {
          name: WRITE_FILE_TOOL_NAME.to_string(),
          description:
              "Create or overwrite a file. Use this for simple single-file writes instead of apply_patch; \
               for multi-file changes, deletions, or moves use apply_patch.".to_string(),
          strict: false,
          defer_loading: None,
          parameters: JsonSchema::object(
              properties,
              Some(vec!["path".to_string(), "content".to_string()]),
              Some(false.into()),
          ),
          output_schema: None,
      })
  }
  ```
- [ ] Implement `create_edit_file_tool`:
  ```rust
  pub fn create_edit_file_tool(options: FileToolOptions) -> ToolSpec {
      let mut properties = BTreeMap::from([
          (
              "path".to_string(),
              JsonSchema::string(Some(
                  "Path to the file to edit. Relative paths are resolved against the working directory.".to_string(),
              )),
          ),
          (
              "old_string".to_string(),
              JsonSchema::string(Some("Exact string to replace. Must not be empty.".to_string())),
          ),
          (
              "new_string".to_string(),
              JsonSchema::string(Some("Replacement string.".to_string())),
          ),
          (
              "replace_all".to_string(),
              JsonSchema::boolean(Some(
                  "If true, replace all occurrences of old_string. Otherwise the call errors when old_string appears more than once.".to_string(),
              )),
          ),
      ]);
      environment_id_property(&mut properties, options);
      ToolSpec::Function(ResponsesApiTool {
          name: EDIT_FILE_TOOL_NAME.to_string(),
          description:
              "Edit a file by replacing an exact string. Use this for simple single-file edits instead of apply_patch; \
               for multi-file changes, deletions, or moves use apply_patch.".to_string(),
          strict: false,
          defer_loading: None,
          parameters: JsonSchema::object(
              properties,
              Some(vec!["path".to_string(), "old_string".to_string(), "new_string".to_string()]),
              Some(false.into()),
          ),
          output_schema: None,
      })
  }
  ```
- [ ] Run the spec tests:
  ```bash
  cargo test -p ody-core file_tools_spec
  ```
  Expected: existing tests pass; new functions compile.
- [ ] Add spec tests at the bottom of `file_tools_spec.rs`:
  ```rust
  #[test]
  fn write_file_requires_path_and_content() {
      let json = spec_json(&create_write_file_tool(FileToolOptions::default()));
      assert!(
          json.contains("\"required\":[") && json.contains("\"path\"") && json.contains("\"content\""),
          "write_file must require path and content: {json}"
      );
  }

  #[test]
  fn write_file_advertises_simple_over_apply_patch() {
      let json = spec_json(&create_write_file_tool(FileToolOptions::default()));
      assert!(
          json.contains("simple single-file writes instead of apply_patch"),
          "write_file description should push simple writes away from apply_patch: {json}"
      );
  }

  #[test]
  fn edit_file_rejects_empty_old_string_in_description() {
      let json = spec_json(&create_edit_file_tool(FileToolOptions::default()));
      assert!(
          json.contains("Must not be empty"),
          "edit_file description must tell the model old_string cannot be empty: {json}"
      );
  }

  #[test]
  fn edit_file_replace_all_is_optional() {
      let json = spec_json(&create_edit_file_tool(FileToolOptions::default()));
      assert!(
          !json.contains("\"required\":[") || !json.contains("\"replace_all\""),
          "replace_all must be optional: {json}"
      );
  }
  ```
- [ ] Run the new tests:
  ```bash
  cargo test -p ody-core file_tools_spec::tests::write_file
  cargo test -p ody-core file_tools_spec::tests::edit_file
  ```
  Expected: four tests pass.
- [ ] Commit:
  ```bash
  git add core/src/tools/handlers/file_tools_spec.rs
  git commit -m "feat(file_tools): add write_file and edit_file tool specs"
  ```

### Task 2: Add shared write/edit helpers in `file_tools/write_edit.rs`

**Depends on:** Task 1

**Files:**
- Create: `core/src/tools/handlers/file_tools/write_edit.rs`
- Modify: `core/src/tools/handlers/apply_patch.rs:401` (make `write_permissions_for_paths` `pub(crate)`)

**Implementation:**
- [ ] Make `write_permissions_for_paths` reusable. In `core/src/tools/handlers/apply_patch.rs:401`, change:
  ```rust
  fn write_permissions_for_paths(...)
  ```
  to:
  ```rust
  pub(crate) fn write_permissions_for_paths(...)
  ```
  This is safe: it only exposes the existing permission-computation helper.
- [ ] Create `core/src/tools/handlers/file_tools/write_edit.rs`:
  ```rust
  use super::PathAccessMode;
  use super::local_search_root;
  use crate::function_tool::FunctionCallError;
  use crate::session::session::Session;
  use crate::session::turn_context::TurnContext;
  use crate::tools::handlers::EffectiveAdditionalPermissions;
  use crate::tools::handlers::apply_granted_turn_permissions;
  use crate::tools::handlers::apply_patch::write_permissions_for_paths;
  use ody_file_system::CreateDirectoryOptions;
  use ody_file_system::FileSystemSandboxContext;
  use ody_protocol::models::AdditionalPermissionProfile;
  use ody_protocol::permissions::FileSystemSandboxPolicy;
  use ody_protocol::protocol::FileChange;
  use ody_sandboxing::policy_transforms::effective_file_system_sandbox_policy;
  use ody_sandboxing::policy_transforms::merge_permission_profiles;
  use ody_utils_absolute_path::AbsolutePathBuf;
  use ody_utils_path_uri::PathUri;
  use serde::Deserialize;
  use similar::TextDiff;
  use std::collections::HashMap;
  use std::path::Path;
  use std::path::PathBuf;

  pub const MAX_WRITE_FILE_BYTES: usize = 10 * 1024 * 1024;
  pub const MAX_FILE_SIZE_FOR_DIFF: usize = 1024 * 1024;

  #[derive(Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq)]
  pub enum WriteMode {
      #[default]
      Overwrite,
      Append,
  }

  #[derive(Deserialize)]
  pub struct WriteFileArgs {
      pub path: String,
      pub content: String,
      #[serde(default)]
      pub mode: WriteMode,
      #[serde(default)]
      pub environment_id: Option<String>,
  }

  #[derive(Deserialize)]
  pub struct EditFileArgs {
      pub path: String,
      pub old_string: String,
      pub new_string: String,
      #[serde(default)]
      pub replace_all: bool,
      #[serde(default)]
      pub environment_id: Option<String>,
  }

  /// Resolves and confines the target path, rejecting remote environments.
  pub fn resolve_write_path(
      turn: &TurnContext,
      environment_id: Option<&str>,
      path: &str,
  ) -> Result<AbsolutePathBuf, FunctionCallError> {
      local_search_root(turn, environment_id, Some(path), PathAccessMode::WorkspaceRelativeOnly)
  }

  /// Computes effective permissions for writing to `path`.
  pub async fn ensure_write_permissions(
      session: &Session,
      turn: &TurnContext,
      environment_id: &str,
      path: &PathUri,
      cwd: &PathUri,
  ) -> Result<EffectiveAdditionalPermissions, FunctionCallError> {
      let native_cwd = cwd.to_abs_path().map_err(|err| {
          FunctionCallError::RespondToModel(format!(
              "environment cwd `{}` is not native to the Ody host: {err}",
              cwd
          ))
      })?;
      let native_path = path.to_abs_path().map_err(|err| {
          FunctionCallError::RespondToModel(format!(
              "path `{}` is not native to the Ody host: {err}",
              path
          ))
      })?;

      let granted_permissions = merge_permission_profiles(
          session.granted_session_permissions(environment_id).await.as_ref(),
          session.granted_turn_permissions(environment_id).await.as_ref(),
      );
      let base_file_system_sandbox_policy = turn.file_system_sandbox_policy();
      let file_system_sandbox_policy = effective_file_system_sandbox_policy(
          &base_file_system_sandbox_policy,
          granted_permissions.as_ref(),
      );
      let effective_additional_permissions = apply_granted_turn_permissions(
          session,
          environment_id,
          native_cwd.as_path(),
          crate::sandboxing::SandboxPermissions::UseDefault,
          write_permissions_for_paths(
              &[native_path],
              &file_system_sandbox_policy,
              &native_cwd,
          ),
      )
      .await;

      Ok(effective_additional_permissions)
  }

  pub fn sandbox_context_for_write(
      turn: &TurnContext,
      cwd: &PathUri,
      additional_permissions: Option<AdditionalPermissionProfile>,
  ) -> FileSystemSandboxContext {
      turn.file_system_sandbox_context(additional_permissions, cwd)
  }

  pub async fn atomic_write(
      fs: &dyn ody_exec_server::ExecutorFileSystem,
      path: &PathUri,
      content: Vec<u8>,
      sandbox: Option<&FileSystemSandboxContext>,
  ) -> Result<(), FunctionCallError> {
      let temp_path = temp_path_for(path);
      fs.write_file(&temp_path, content.clone(), sandbox).await.map_err(|err| {
          FunctionCallError::RespondToModel(format!(
              "unable to write temporary file `{}`: {err}",
              temp_path
          ))
      })?;
      match fs.rename(&temp_path, path, sandbox).await {
          Ok(()) => Ok(()),
          Err(rename_err) => {
              let _ = fs
                  .remove(
                      &temp_path,
                      ody_file_system::RemoveOptions {
                          recursive: false,
                          force: true,
                      },
                      sandbox,
                  )
                  .await;
              fs.write_file(path, content, sandbox).await.map_err(|err| {
                  FunctionCallError::RespondToModel(format!(
                      "unable to write `{}` (rename also failed: {rename_err}): {err}",
                      path
                  ))
              })
          }
      }
  }

  fn temp_path_for(path: &PathUri) -> PathUri {
      let suffix = uuid::Uuid::new_v4().to_string();
      let mut buf = path.to_path_buf();
      let file_name = buf
          .file_name()
          .map(|s| s.to_os_string())
          .unwrap_or_else(|| std::ffi::OsString::from("file"));
      let new_name = format!("{}.ody-write-tmp-{}", file_name.to_string_lossy(), suffix);
      buf.set_file_name(new_name);
      PathUri::from_host_native_path(buf).expect("temp path is absolute")
  }

  pub fn unified_diff_for_update(original: &str, updated: &str) -> String {
      TextDiff::from_lines(original, updated)
          .unified_diff()
          .context_radius(3)
          .to_string()
  }

  pub fn non_overlapping_occurrences(haystack: &str, needle: &str) -> usize {
      assert!(!needle.is_empty());
      let mut count = 0;
      let mut start = 0;
      while let Some(pos) = haystack[start..].find(needle) {
          count += 1;
          start += pos + needle.len();
      }
      count
  }

  pub fn near_text_hint(original: &str, old_string: &str) -> String {
      if original.is_empty() {
          return "(file is empty)".to_string();
      }
      if original.len() > MAX_FILE_SIZE_FOR_DIFF {
          return "(file too large to generate hint)".to_string();
      }
      let first_line = old_string
          .lines()
          .find(|l| !l.trim().is_empty())
          .unwrap_or(old_string);
      let lines: Vec<&str> = original.lines().collect();
      let mut candidates: Vec<usize> = lines
          .iter()
          .enumerate()
          .filter(|(_, line)| line.contains(first_line))
          .map(|(idx, _)| idx)
          .collect();
      if candidates.is_empty() {
          candidates = (0..lines.len()).collect();
      }
      let best = candidates
          .into_iter()
          .max_by_key(|idx| common_prefix_len(old_string, lines[*idx]))
          .unwrap_or(0);
      let start = best.saturating_sub(3);
      let end = (best + 4).min(lines.len());
      let context = lines[start..end].join("\n");
      format!("Closest match at line {}:\n```\n{}\n```", best + 1, context)
  }

  fn common_prefix_len(a: &str, b: &str) -> usize {
      a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
  }

  pub async fn ensure_parent_directory(
      fs: &dyn ody_exec_server::ExecutorFileSystem,
      path: &PathUri,
      sandbox: Option<&FileSystemSandboxContext>,
  ) -> Result<(), FunctionCallError> {
      let parent = path.to_path_buf().parent().map(Path::to_path_buf);
      let Some(parent) = parent else {
          return Ok(());
      };
      let parent_uri = PathUri::from_host_native_path(parent).map_err(|err| {
          FunctionCallError::RespondToModel(format!("parent path is not usable: {err}"))
      })?;
      match fs.get_metadata(&parent_uri, sandbox).await {
          Ok(metadata) if metadata.is_file => Err(FunctionCallError::RespondToModel(format!(
              "parent path `{}` is a file, cannot create directory",
              parent_uri
          ))),
          Ok(_) => Ok(()),
          Err(_) => fs
              .create_directory(
                  &parent_uri,
                  CreateDirectoryOptions { recursive: true },
                  sandbox,
              )
              .await
              .map_err(|err| {
                  FunctionCallError::RespondToModel(format!(
                      "unable to create parent directory `{}`: {err}",
                      parent_uri
                  ))
              }),
      }
  }

  pub async fn file_change_from_write(
      fs: &dyn ody_exec_server::ExecutorFileSystem,
      path: &PathUri,
      sandbox: Option<&FileSystemSandboxContext>,
      new_content: &str,
      original_existed: bool,
  ) -> Result<FileChange, FunctionCallError> {
      if !original_existed {
          return Ok(FileChange::Add {
              content: new_content.to_string(),
          });
      }
      let original = match fs.read_file_text(path, sandbox).await {
          Ok(text) if text.len() <= MAX_FILE_SIZE_FOR_DIFF => text,
          Ok(_) | Err(_) => {
              return Ok(FileChange::Update {
                  unified_diff: "(diff omitted: file too large)".to_string(),
                  move_path: None,
              });
          }
      };
      Ok(FileChange::Update {
          unified_diff: unified_diff_for_update(&original, new_content),
          move_path: None,
      })
  }
  ```
  Note: `uuid` must already be a workspace dependency. If not, add `uuid` to `core/Cargo.toml` under `[dependencies]` or use an existing internal id generator.
  Note: `uuid` must already be a workspace dependency. If not, add `uuid` to `core/Cargo.toml` under `[dependencies]` or use an existing internal id generator.
- [ ] Add a build/typecheck step to ensure the new module compiles:
  ```bash
  cargo check -p ody-core --lib
  ```
  Expected: no compile errors.
- [ ] Commit:
  ```bash
  git add core/src/tools/handlers/file_tools/write_edit.rs core/src/tools/handlers/apply_patch.rs
  git commit -m "feat(file_tools): add shared write/edit helpers and permissions"
  ```

### Task 3: Implement `WriteFileHandler` in `file_tools/write.rs`

**Depends on:** Task 1, Task 2, Part 2: Task 6

**Files:**
- Create: `core/src/tools/handlers/file_tools/write.rs`

**Implementation:**
- [ ] Create `core/src/tools/handlers/file_tools/write.rs`:
  ```rust
  use super::write_edit::MAX_WRITE_FILE_BYTES;
  use super::write_edit::WriteFileArgs;
  use super::write_edit::WriteMode;
  use super::write_edit::atomic_write;
  use super::write_edit::ensure_parent_directory;
  use super::write_edit::ensure_write_permissions;
  use super::write_edit::file_change_from_write;
  use super::write_edit::resolve_write_path;
  use super::write_edit::sandbox_context_for_write;
  use crate::function_tool::FunctionCallError;
  use crate::tools::context::FunctionToolOutput;
  use crate::tools::context::ToolInvocation;
  use crate::tools::context::ToolOutput;
  use crate::tools::context::ToolPayload;
  use crate::tools::context::boxed_tool_output;
  use crate::tools::events::ToolEmitter;
  use crate::tools::events::ToolEventCtx;
  use crate::tools::events::ToolEventStage;
  use crate::tools::handlers::file_tools_spec::WRITE_FILE_TOOL_NAME;
  use crate::tools::handlers::file_tools_spec::create_write_file_tool;
  use crate::tools::handlers::file_tools_spec::FileToolOptions;
  use crate::tools::handlers::parse_arguments;
  use crate::tools::handlers::resolve_tool_environment;
  use crate::tools::registry::CoreToolRuntime;
  use crate::tools::registry::ToolExecutor;
  use ody_exec_server::ExecToolCallOutput;
  use ody_protocol::exec_output::StreamOutput;
  use ody_tools::ToolName;
  use ody_tools::ToolSpec;
  use ody_utils_path_uri::PathUri;

  #[derive(Default)]
  pub struct WriteFileHandler {
      options: FileToolOptions,
  }

  impl WriteFileHandler {
      pub fn new(options: FileToolOptions) -> Self {
          Self { options }
      }
  }

  impl ToolExecutor<ToolInvocation> for WriteFileHandler {
      fn tool_name(&self) -> ToolName {
          ToolName::plain(WRITE_FILE_TOOL_NAME)
      }

      fn spec(&self) -> ToolSpec {
          create_write_file_tool(self.options)
      }

      fn supports_parallel_tool_calls(&self) -> bool {
          true
      }

      fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
          Box::pin(self.handle_call(invocation))
      }
  }

  impl CoreToolRuntime for WriteFileHandler {}

  impl WriteFileHandler {
      async fn handle_call(
          &self,
          invocation: ToolInvocation,
      ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
          let ToolInvocation {
              session,
              turn,
              call_id,
              tracker,
              payload,
              ..
          } = invocation;

          let ToolPayload::Function { arguments } = payload else {
              return Err(FunctionCallError::RespondToModel(
                  "write_file handler received unsupported payload".to_string(),
              ));
          };
          let args: WriteFileArgs = parse_arguments(&arguments)?;
          let abs_path =
              resolve_write_path(turn.as_ref(), args.environment_id.as_deref(), &args.path)?;
          let path_uri = PathUri::from_abs_path(&abs_path);

          let Some(turn_environment) =
              resolve_tool_environment(turn.as_ref(), args.environment_id.as_deref())?
          else {
              return Err(FunctionCallError::RespondToModel(
                  "write_file is unavailable in this session".to_string(),
              ));
          };
          let cwd = turn_environment.cwd();
          let fs = turn_environment.environment.get_filesystem();

          let effective_permissions = ensure_write_permissions(
              session.as_ref(),
              turn.as_ref(),
              &turn_environment.environment_id,
              &path_uri,
              cwd,
          )
          .await?;
          if !effective_permissions.permissions_preapproved {
              return Err(FunctionCallError::RespondToModel(format!(
                  "write_file to `{}` requires write permission for its parent directory. \
                   Use request_permissions to grant file_system write access first.",
                  abs_path.as_path().display()
              )));
          }
          let sandbox = sandbox_context_for_write(
              turn.as_ref(),
              cwd,
              effective_permissions.additional_permissions,
          );

          let content_bytes = args.content.into_bytes();
          if content_bytes.len() > MAX_WRITE_FILE_BYTES {
              return Err(FunctionCallError::RespondToModel(format!(
                  "content exceeds {} MiB limit",
                  MAX_WRITE_FILE_BYTES / 1024 / 1024
              )));
          }

          let original_existed = fs
              .get_metadata(&path_uri, Some(&sandbox))
              .await
              .is_ok_and(|m| m.is_file);

          match args.mode {
              WriteMode::Overwrite => {
                  ensure_parent_directory(fs.as_ref(), &path_uri, Some(&sandbox)).await?;
                  atomic_write(fs.as_ref(), &path_uri, content_bytes, Some(&sandbox)).await?;
              }
              WriteMode::Append => {
                  ensure_parent_directory(fs.as_ref(), &path_uri, Some(&sandbox)).await?;
                  let original = fs
                      .read_file_text(&path_uri, Some(&sandbox))
                      .await
                      .unwrap_or_default();
                  if original.len() + content_bytes.len()
                      > super::write_edit::MAX_FILE_SIZE_FOR_DIFF
                  {
                      return Err(FunctionCallError::RespondToModel(
                          "append would exceed safe file size for diff generation".to_string(),
                      ));
                  }
                  let mut new_content = original;
                  new_content.push_str(&String::from_utf8_lossy(&content_bytes));
                  atomic_write(fs.as_ref(), &path_uri, new_content.into_bytes(), Some(&sandbox))
                      .await?;
              }
          }

          let file_change = file_change_from_write(
              fs.as_ref(),
              &path_uri,
              Some(&sandbox),
              &String::from_utf8_lossy(&content_bytes),
              original_existed || args.mode == WriteMode::Append,
          )
          .await?;
          let changes = std::collections::HashMap::from([(
              path_uri.to_path_buf(),
              file_change,
          )]);
          let emitter = ToolEmitter::apply_patch_for_environment(
              changes,
              effective_permissions.permissions_preapproved,
              turn_environment.environment_id.clone(),
          );
          let event_ctx = ToolEventCtx::new(
              session.as_ref(),
              turn.as_ref(),
              &call_id,
              Some(&tracker),
          );
          emitter.begin(event_ctx).await;
          let output = ExecToolCallOutput {
              exit_code: 0,
              stdout: StreamOutput::from_text(String::new()),
              stderr: StreamOutput::from_text(String::new()),
              ..Default::default()
          };
          emitter
              .emit(
                  event_ctx,
                  ToolEventStage::Success {
                      output,
                      applied_patch_delta: None,
                  },
              )
              .await;

          let message = format!(
              "Wrote {} bytes to {}",
              content_bytes.len(),
              abs_path.as_path().display()
          );
          Ok(boxed_tool_output(FunctionToolOutput::from_text(
              message,
              Some(true),
          )))
      }
  }

  #[cfg(test)]
  mod tests;
  ```
- [ ] Run typecheck:
  ```bash
  cargo check -p ody-core --lib
  ```
  Expected: `WriteFileHandler` compiles.
- [ ] Commit:
  ```bash
  git add core/src/tools/handlers/file_tools/write.rs
  git commit -m "feat(file_tools): add WriteFileHandler"
  ```

### Task 4: Implement `EditFileHandler` in `file_tools/edit.rs`

**Depends on:** Task 1, Task 2, Part 2: Task 6

**Files:**
- Create: `core/src/tools/handlers/file_tools/edit.rs`

**Implementation:**
- [ ] Create `core/src/tools/handlers/file_tools/edit.rs`:
  ```rust
  use super::write_edit::EditFileArgs;
  use super::write_edit::MAX_FILE_SIZE_FOR_DIFF;
  use super::write_edit::atomic_write;
  use super::write_edit::ensure_write_permissions;
  use super::write_edit::near_text_hint;
  use super::write_edit::non_overlapping_occurrences;
  use super::write_edit::resolve_write_path;
  use super::write_edit::sandbox_context_for_write;
  use super::write_edit::unified_diff_for_update;
  use crate::function_tool::FunctionCallError;
  use crate::tools::context::FunctionToolOutput;
  use crate::tools::context::ToolInvocation;
  use crate::tools::context::ToolOutput;
  use crate::tools::context::ToolPayload;
  use crate::tools::context::boxed_tool_output;
  use crate::tools::events::ToolEmitter;
  use crate::tools::events::ToolEventCtx;
  use crate::tools::events::ToolEventStage;
  use crate::tools::handlers::file_tools_spec::EDIT_FILE_TOOL_NAME;
  use crate::tools::handlers::file_tools_spec::FileToolOptions;
  use crate::tools::handlers::file_tools_spec::create_edit_file_tool;
  use crate::tools::handlers::parse_arguments;
  use crate::tools::handlers::resolve_tool_environment;
  use crate::tools::registry::CoreToolRuntime;
  use crate::tools::registry::ToolExecutor;
  use ody_exec_server::ExecToolCallOutput;
  use ody_protocol::exec_output::StreamOutput;
  use ody_protocol::protocol::FileChange;
  use ody_tools::ToolName;
  use ody_tools::ToolSpec;
  use ody_utils_path_uri::PathUri;

  #[derive(Default)]
  pub struct EditFileHandler {
      options: FileToolOptions,
  }

  impl EditFileHandler {
      pub fn new(options: FileToolOptions) -> Self {
          Self { options }
      }
  }

  impl ToolExecutor<ToolInvocation> for EditFileHandler {
      fn tool_name(&self) -> ToolName {
          ToolName::plain(EDIT_FILE_TOOL_NAME)
      }

      fn spec(&self) -> ToolSpec {
          create_edit_file_tool(self.options)
      }

      fn supports_parallel_tool_calls(&self) -> bool {
          true
      }

      fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
          Box::pin(self.handle_call(invocation))
      }
  }

  impl CoreToolRuntime for EditFileHandler {}

  impl EditFileHandler {
      async fn handle_call(
          &self,
          invocation: ToolInvocation,
      ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
          let ToolInvocation {
              session,
              turn,
              call_id,
              tracker,
              payload,
              ..
          } = invocation;

          let ToolPayload::Function { arguments } = payload else {
              return Err(FunctionCallError::RespondToModel(
                  "edit_file handler received unsupported payload".to_string(),
              ));
          };
          let args: EditFileArgs = parse_arguments(&arguments)?;
          if args.old_string.is_empty() {
              return Err(FunctionCallError::RespondToModel(
                  "old_string cannot be empty".to_string(),
              ));
          }
          if args.old_string == args.new_string {
              return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                  "No changes needed".to_string(),
                  Some(true),
              )));
          }

          let abs_path =
              resolve_write_path(turn.as_ref(), args.environment_id.as_deref(), &args.path)?;
          let path_uri = PathUri::from_abs_path(&abs_path);

          let Some(turn_environment) =
              resolve_tool_environment(turn.as_ref(), args.environment_id.as_deref())?
          else {
              return Err(FunctionCallError::RespondToModel(
                  "edit_file is unavailable in this session".to_string(),
              ));
          };
          let cwd = turn_environment.cwd();
          let fs = turn_environment.environment.get_filesystem();

          let effective_permissions = ensure_write_permissions(
              session.as_ref(),
              turn.as_ref(),
              &turn_environment.environment_id,
              &path_uri,
              cwd,
          )
          .await?;
          if !effective_permissions.permissions_preapproved {
              return Err(FunctionCallError::RespondToModel(format!(
                  "edit_file to `{}` requires write permission for its parent directory. \
                   Use request_permissions to grant file_system write access first.",
                  abs_path.as_path().display()
              )));
          }
          let sandbox = sandbox_context_for_write(
              turn.as_ref(),
              cwd,
              effective_permissions.additional_permissions,
          );

          let original = fs
              .read_file_text(&path_uri, Some(&sandbox))
              .await
              .map_err(|err| {
                  FunctionCallError::RespondToModel(format!(
                      "cannot edit `{}`: {err}",
                      abs_path.as_path().display()
                  ))
              })?;

          if original.len() > MAX_FILE_SIZE_FOR_DIFF {
              return Err(FunctionCallError::RespondToModel(format!(
                  "`{}` is too large to edit with edit_file",
                  abs_path.as_path().display()
              )));
          }

          let count = non_overlapping_occurrences(&original, &args.old_string);
          if count == 0 {
              let hint = near_text_hint(&original, &args.old_string);
              return Err(FunctionCallError::RespondToModel(format!(
                  "old_string not found in `{}`. Did you mean:\n{hint}",
                  abs_path.as_path().display()
              )));
          }
          if count > 1 && !args.replace_all {
              return Err(FunctionCallError::RespondToModel(format!(
                  "old_string appears {count} times in `{}`. Set replace_all=true or use apply_patch.",
                  abs_path.as_path().display()
              )));
          }

          let new_content = if args.replace_all {
              original.replace(&args.old_string, &args.new_string)
          } else {
              original.replacen(&args.old_string, &args.new_string, 1)
          };
          atomic_write(
              fs.as_ref(),
              &path_uri,
              new_content.clone().into_bytes(),
              Some(&sandbox),
          )
          .await?;

          let changes = std::collections::HashMap::from([(
              path_uri.to_path_buf(),
              FileChange::Update {
                  unified_diff: unified_diff_for_update(&original, &new_content),
                  move_path: None,
              },
          )]);
          let emitter = ToolEmitter::apply_patch_for_environment(
              changes,
              effective_permissions.permissions_preapproved,
              turn_environment.environment_id.clone(),
          );
          let event_ctx = ToolEventCtx::new(
              session.as_ref(),
              turn.as_ref(),
              &call_id,
              Some(&tracker),
          );
          emitter.begin(event_ctx).await;
          let output = ExecToolCallOutput {
              exit_code: 0,
              stdout: StreamOutput::from_text(String::new()),
              stderr: StreamOutput::from_text(String::new()),
              ..Default::default()
          };
          emitter
              .emit(
                  event_ctx,
                  ToolEventStage::Success {
                      output,
                      applied_patch_delta: None,
                  },
              )
              .await;

          let message = format!(
              "Replaced {} occurrence(s) in {}",
              count,
              abs_path.as_path().display()
          );
          Ok(boxed_tool_output(FunctionToolOutput::from_text(
              message,
              Some(true),
          )))
      }
  }

  #[cfg(test)]
  mod tests;
  ```
- [ ] Run typecheck:
  ```bash
  cargo check -p ody-core --lib
  ```
  Expected: `EditFileHandler` compiles.
- [ ] Commit:
  ```bash
  git add core/src/tools/handlers/file_tools/edit.rs
  git commit -m "feat(file_tools): add EditFileHandler"
  ```

### Task 5: Wire handlers and register in turn plan

**Depends on:** Task 3, Task 4

**Files:**
- Modify: `core/src/tools/handlers/file_tools/mod.rs`
- Modify: `core/src/tools/handlers/mod.rs`
- Modify: `core/src/tools/spec_plan.rs`

**Implementation:**
- [ ] In `core/src/tools/handlers/file_tools/mod.rs`, add module declarations and re-exports:
  ```rust
  mod edit;
  mod write;
  mod write_edit;
  // ... existing modules ...
  pub use edit::EditFileHandler;
  pub use write::WriteFileHandler;
  ```
- [ ] In `core/src/tools/handlers/mod.rs`, add public re-exports:
  ```rust
  pub use file_tools::EditFileHandler;
  pub use file_tools::WriteFileHandler;
  ```
- [ ] In `core/src/tools/spec_plan.rs`, update `add_file_tools` (around line 660) to register the new handlers alongside read_file/grep/glob/jq:
  ```rust
  planned_tools.add(ReadFileHandler::new(options));
  planned_tools.add(GrepHandler::new(options));
  planned_tools.add(GlobHandler::new(options));
  planned_tools.add(JqHandler::new(options));
  planned_tools.add(WriteFileHandler::new(options));
  planned_tools.add(EditFileHandler::new(options));
  ```
- [ ] Add a registration assertion verifying both tools appear in the planned tool list when `Feature::FileTools` is enabled. In `core/src/tools/spec_plan.rs` tests (or a new `core/src/tools/spec_plan_tests.rs`):
  ```rust
  #[tokio::test]
  async fn file_tools_include_write_and_edit() {
      let registry = build_registry_for_test(/* enable FileTools */).await;
      let names: Vec<String> = registry
          .tool_names_for_test()
          .iter()
          .map(|n| n.to_string())
          .collect();
      assert!(names.contains(&"write_file".to_string()));
      assert!(names.contains(&"edit_file".to_string()));
  }
  ```
  (Use the same harness pattern as existing `spec_plan` tests; adjust helper names to match the actual code.)
- [ ] Run typecheck for the workspace:
  ```bash
  cargo check --workspace --all-targets
  ```
  Expected: no compile errors.
- [ ] Run core tests:
  ```bash
  cargo test -p ody-core --tests
  ```
  Expected: existing tests still pass.
- [ ] Commit:
  ```bash
  git add core/src/tools/handlers/file_tools/mod.rs core/src/tools/handlers/mod.rs core/src/tools/spec_plan.rs
  git commit -m "feat(file_tools): wire write_file and edit_file handlers into registry"
  ```

## Self-review (Part 1)

- [ ] 1. Spec-coverage: Part 1 covers `write_file`/`edit_file` schema, handler, and registration requirements from the design.
- [ ] 2. Placeholder scan: no TODO/TBD/deferred placeholders in task steps.
- [ ] 3. No phantom tasks: each task creates/modifies files and includes a compile/typecheck step.
- [ ] 4. Dependency soundness: Task 5 depends on Task 3/4; Task 3/4 depend on Task 1/2; Task 2 depends on Task 1.
- [ ] 5. Caller & build soundness: Task 5 ends with `cargo check --workspace --all-targets` after modifying shared exports and registration.
- [ ] 6. Test-the-risk: Task 1 includes spec assertions; Task 5 includes registration assertion. Handler behavioral tests live in the files created in Task 3/4 (test modules to be filled in Part 3).
- [ ] 7. Type consistency: `WriteFileArgs`/`EditFileArgs`/`WriteMode` defined in Task 2 are used by Task 3/4; `ToolEmitter`/`FileChange` types match Part 2 definitions.

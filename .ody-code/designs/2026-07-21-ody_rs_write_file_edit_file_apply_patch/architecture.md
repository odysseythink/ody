## Part 1: Architecture — Handlers, Spec, and Integration

This part describes the concrete handlers, tool schemas, and how the new tools wire into the existing `file_tools` subsystem and tool registry.

### 1.1 Reuse anchors

| Existing component | Path | What we reuse it for |
|-------------------|------|---------------------|
|`FileToolOptions` | `core/src/tools/handlers/file_tools_spec.rs:34` | Schema generation options |
|`local_search_root` / `confine_to_root` | `core/src/tools/handlers/file_tools/mod.rs:40-115` | Workspace path boundary |
|`ToolExecutor<ToolInvocation>` | `core/src/tools/registry.rs:44` | Handler trait contract |
|`FunctionToolOutput` | `core/src/tools/context.rs:184` | Model-facing output wrapper |
|`ToolEmitter` / `FileChange` | `core/src/tools/events.rs:123` | Diff event emission |
|`SharedTurnDiffTracker` | `core/src/tools/context.rs:38` | Per-turn diff tracking |
|`apply_granted_turn_permissions` | `core/src/tools/handlers/mod.rs:268` | Sandbox permission computation |
|`write_permissions_for_paths` | `core/src/tools/handlers/apply_patch.rs:401` | Missing write permission computation |
|`ExecutorFileSystem` | `file-system/src/lib.rs:191-238` | File I/O primitives |

### 1.2 New tool specs

Constants added in `core/src/tools/handlers/file_tools_spec.rs`:

```rust
pub const WRITE_FILE_TOOL_NAME: &str = "write_file";
pub const EDIT_FILE_TOOL_NAME: &str = "edit_file";
pub const MAX_WRITE_FILE_BYTES: usize = 10 * 1024 * 1024; // 10 MiB
```

`write_file` parameters: `path` (required), `content` (required, capped at 10 MiB), `mode` (enum `"overwrite"` / `"append"`, default `"overwrite"`).

`edit_file` parameters: `path` (required), `old_string` (required, min length 1), `new_string` (required), `replace_all` (optional boolean).

### 1.3 New handlers

Create `core/src/tools/handlers/file_tools/write.rs` and `core/src/tools/handlers/file_tools/edit.rs`.

Both implement `ToolExecutor<ToolInvocation>` following the pattern of `ReadFileHandler`.

`WriteFileHandler` pseudocode:

```rust
fn handle(invocation) {
    args = parse_arguments();
    if args.content.len() > MAX_WRITE_FILE_BYTES { error; }
    abs_path = local_search_root(args.path)?;
    request_write_approval_if_needed(abs_path).await?; // first write to this path
    parent = abs_path.parent();
    if parent_is_file(parent) { error; }
    if !parent.exists() { fs.create_directory(parent, recursive=true).await?; }
    new_content = match args.mode {
        Append => read_existing_or_empty(abs_path) + args.content,
        Overwrite => args.content,
    };
    atomic_write(fs, abs_path, new_content).await?;
    emit_file_change_event(abs_path, new_content, ToolSource::WriteFile).await;
    return "Wrote {len} bytes to {path}";
}
```

`EditFileHandler` pseudocode:

```rust
fn handle(invocation) {
    args = parse_arguments();
    if args.old_string == args.new_string { return "No changes needed"; }
    abs_path = local_search_root(args.path)?;
    request_write_approval_if_needed(abs_path).await?;
    original = fs.read_file_text(abs_path).await?;
    count = non_overlapping_occurrences(original, args.old_string);
    if count == 0 { error with near_text_hint; }
    if count > 1 && !args.replace_all { error; }
    new_content = if replace_all { original.replace(...) } else { original.replacen(..., 1) };
    atomic_write(fs, abs_path, new_content).await?;
    emit_file_change_event(abs_path, new_content, ToolSource::EditFile).await;
    return "Replaced {count} occurrence(s)";
}
```

`atomic_write` writes to a temporary file then renames it to the target path. If the filesystem trait does not support `rename`, it falls back to a direct write and documents the loss of crash-atomicity.

### 1.4 Module exports and registration

In `file_tools/mod.rs`:

```rust
mod write;
mod edit;
pub use write::WriteFileHandler;
pub use edit::EditFileHandler;
```

In `handlers/mod.rs` re-export both handlers. Register them in the tool registry alongside `ReadFileHandler`/`GrepHandler`/`GlobHandler`.

### 1.5 Event emission strategy

Emit a `FileChange` event after every successful write/edit. The payload is the same `TurnItem::FileChange` used by `apply_patch`, so the UI consumes it unchanged. We also attach a source tag (`write_file` / `edit_file`) to the event metadata so telemetry can measure tool switching.

### 1.6 Relationship with `apply_patch`

Update `apply_patch` description to tell the model to prefer `write_file`/`edit_file` for simple single-file operations, and reserve `apply_patch` for multi-file, delete, move, or atomic changes.

### 1.7 Apply_patch error-message improvement

Improve parser diagnostics in `apply-patch/src/parser.rs` for:

- `+*** End Patch` and `+*** End of File` markers.
- `*** End of File` used outside an `Update File` hunk.
- Missing `*** End Patch`.

The grammar itself is unchanged.

### 1.8 Security approval helper

`request_write_approval_if_needed` reuses `write_permissions_for_paths` and `apply_granted_turn_permissions` (same as `apply_patch`) but for a single path. The first write to any path (overwrite, append, or edit) requires approval; subsequent writes to the same path are pre-approved within the turn.

### 1.9 Open questions

None.

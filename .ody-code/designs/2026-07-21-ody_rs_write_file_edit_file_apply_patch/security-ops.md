## Part 3: Security, Observability, Rollout, and Tests

### 3.1 Security design

#### 3.1.1 Trust boundary and path confinement

Both tools reuse `local_search_root` -> `confine_to_root`, which lexically normalizes the path and rejects anything outside the workspace.

#### 3.1.2 Sandbox approval for writes

We reuse the permission model from `apply_patch` (`apply_patch.rs:401-431`, `mod.rs:268-312`) but applied to a single path. The rule is: **the first write to any path (overwrite, append, or edit) requires approval; subsequent writes to the same path within the turn are pre-approved.**

This is a revision from the original "first write/overwrite requires approval; append/edit auto-allowed" decision, made after adversarial review pointed out that append and edit can change content as much as overwrite, so treating them differently would be inconsistent.

#### 3.1.3 Abuse cases

| Abuse case | Mitigation |
|---|---|
| Overwrite outside workspace | `confine_to_root` rejects |
| Write to unapproved path | `write_permissions_for_paths` + `apply_granted_turn_permissions` escalates for first write |
| Append to huge file | No explicit disk cap; 10 MiB per-call content limit on `write_file` |
| Edit injects malicious content | Same trust level as `apply_patch`; model controls content |
| Binary corruption via `edit_file` | `edit_file` reads UTF-8 and fails on non-text |
| Directory spraying | Auto-creation is recursive; we check parent is not a file first |

### 3.2 Observability

#### 3.2.1 Events

Emit one `FileChange` event per successful write/edit. The payload is structurally identical to `apply_patch` events. A `source` tag (`write_file` / `edit_file`) is attached to event metadata for telemetry and audit.

#### 3.2.2 Logging and metrics

Both tools follow the existing `ToolExecutor` logging path. No new metrics are added; we rely on existing tool-call telemetry and expect `apply_patch` error rates to drop.

### 3.3 Operations / rollout

#### 3.3.1 Registry registration

Register `WriteFileHandler` and `EditFileHandler` alongside the existing file tools. Default enabled.

#### 3.3.2 Tool description update

Update `apply_patch` description to push simple single-file writes/edits to `write_file`/`edit_file`.

#### 3.3.3 Backwards compatibility

- `apply_patch` behavior unchanged except error messages.
- No feature flags.
- Existing sessions continue using `apply_patch` if they choose.

#### 3.3.4 Docs

Update `AGENTS.md` and code-mode templates to mention new tools.

### 3.4 Test plan

#### 3.4.1 Unit tests for `write_file`

1. Create new file.
2. Overwrite existing file.
3. Create file in missing parent directory.
4. Append to existing file.
5. Append creates file when missing.
6. Reject path escaping workspace.
7. Reject content > 10 MiB.
8. Reject parent path being a file.
9. Verify `FileChange` event with source `write_file`.

#### 3.4.2 Unit tests for `edit_file`

1. Replace single occurrence.
2. Replace with `replace_all=true`.
3. No-op when `old_string == new_string`.
4. Fail when `old_string` not found and verify hint.
5. Fail when `old_string` appears multiple times and `replace_all=false`.
6. Reject non-UTF-8 files.
7. Verify `FileChange` event with source `edit_file`.

#### 3.4.3 Spec tests

Add tests in `file_tools_spec.rs` for:

1. `write_file` and `edit_file` schemas exist.
2. Required fields are correct.
3. `write_file` mode enum is `overwrite`/`append`.
4. `edit_file` `replace_all` is optional.

#### 3.4.4 Apply_patch parser tests

1. `+*** End Patch` produces improved error.
2. `+*** End of File` produces improved error.
3. `*** End of File` outside `Update File` hunk produces improved error.
4. Normal valid patch still succeeds.

#### 3.4.5 Integration test

End-to-end `write_file` -> `edit_file` -> `read_file` with event verification.

### 3.5 Open questions

None.

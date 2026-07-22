## Part 2: Data Models, Algorithms, and Error Behaviors

### 2.1 Data models

#### Tool input arguments

```rust
#[derive(Deserialize, Default)]
enum WriteMode {
    #[default]
    Overwrite,
    Append,
}

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String, // capped at MAX_WRITE_FILE_BYTES (10 MiB)
    #[serde(default)]
    mode: WriteMode,
    #[serde(default)]
    environment_id: Option<String>,
}

#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    environment_id: Option<String>,
}
```

#### Tool output

Both return `FunctionToolOutput`:

- `write_file` success: `Wrote {len} bytes to {path}`
- `edit_file` success: `Replaced {count} occurrence(s)`
- `edit_file` no-op: `No changes needed`
- Any failure: `FunctionCallError::RespondToModel(message)`

#### Event payload

```rust
FileChange::Add { content: String }                    // new file
FileChange::Update { unified_diff: String, move_path: None } // overwrite, append, or edit
```

The event metadata carries a `source` tag: `write_file` or `edit_file`.

### 2.2 Algorithms

#### 2.2.1 Atomic write helper

```text
function atomic_write(fs, abs_path, content):
    temp_path = abs_path + ".ody-write-tmp"
    fs.write_file(temp_path, content.bytes)
    fs.rename(temp_path, abs_path)
```

If `rename` is unavailable, fall back to `fs.write_file(abs_path, content.bytes)` and note the loss of crash-atomicity.

#### 2.2.2 Write file

```text
function write_file(abs_path, content, mode):
    if content.len() > MAX_WRITE_FILE_BYTES: raise SizeError
    parent = abs_path.parent()
    if parent is a file: raise ParentIsFileError
    if parent does not exist:
        fs.create_directory(parent, recursive=true)
    if mode == Append:
        existing = fs.read_file_text(abs_path).ok_or_default()
        new_content = existing + content
    else:
        new_content = content
    atomic_write(fs, abs_path, new_content)
    return new_content
```

#### 2.2.3 Edit file

```text
function edit_file(abs_path, old_string, new_string, replace_all):
    if old_string == new_string: return NoChanges
    original = fs.read_file_text(abs_path)
    count = non_overlapping_occurrences(original, old_string)
    if count == 0:
        hint = near_text_hint(original, old_string)
        raise Error("old_string not found... Did you mean:\n{hint}")
    if count > 1 and not replace_all:
        raise Error("old_string appears {count} times...")
    if replace_all:
        new_content = original.replace(old_string, new_string)
    else:
        new_content = original.replacen(old_string, new_string, 1)
    atomic_write(fs, abs_path, new_content)
    return new_content
```

#### 2.2.4 Non-overlapping occurrence counting

```rust
fn non_overlapping_occurrences(haystack: &str, needle: &str) -> usize {
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        count += 1;
        start += pos + needle.len();
    }
    count
}
```

This matches `String::replace` / `String::replacen` semantics.

#### 2.2.5 Near-text hint

```text
function near_text_hint(original, old_string):
    if old_string is empty or original is empty: return "(empty)"
    first_line = first non-empty line of old_string
    candidates = lines in original containing first_line as substring
    if candidates empty: candidates = all lines
    best_line = candidate with longest common prefix with old_string
    context = ±3 lines around best_line, capped at 100 lines total
    return "Closest match at line {line}:\n```\n{context}\n```"
```

### 2.3 Error behavior matrix

| Scenario | Tool | Behavior | Message |
|---|---|---|---|
| Path escapes workspace | both | Error | `path X escapes Y` |
| Remote environment | both | Error | `file tools unavailable for remote X` |
| Content > 10 MiB | write_file | Error | `content exceeds 10 MiB limit` |
| Parent is a file | write_file | Error | `parent path X is a file` |
| Parent directory missing | write_file | Auto-create | (silent) |
| Invalid mode | write_file | Error | `mode must be overwrite or append` |
| File does not exist for append | write_file | Create | (silent) |
| File does not exist for edit | edit_file | Error | `cannot edit: X does not exist` |
| old_string not found | edit_file | Error + hint | `old_string not found...` |
| old_string appears multiple times | edit_file | Error | `old_string appears N times...` |
| old_string == new_string | edit_file | No-op | `No changes needed` |
| Sandbox rejects first write | both | Error | (passthrough) |
| I/O failure | both | Error | `unable to write X: ...` |
| Non-UTF-8 file | edit_file | Error | `X is not valid UTF-8` |

### 2.4 Open questions

None.

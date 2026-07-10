## Rigor tier addendum: Task skeleton and test-first implementation

Every task in a rigor-tier plan MUST follow this structure and include concrete, executable steps.

### Task header

Every task starts with:

```
### Task N: <name>

**Depends on:** Task M (or `none` if this is an independent task)

**Files:**
- Create: `path/to/new/file.rs:1-50`
- Modify: `path/to/existing/file.rs:100-150`
- Test: `path/to/test/file.rs`
```

Use this format:
- `Create:` for new files (include expected line range after implementation).
- `Modify:` for existing files (specify line ranges of changes, or omit ranges if changes are scattered).
- `Test:` for test files (may be created or modified in the same task).

### Implementation (testable code — test-first)

For business logic, data transformations, state mutations, filters, validators, and math:

- [ ] Write the failing test (show the actual test code, not pseudocode).
- [ ] Run it and verify it FAILS (give the exact command + show the expected failure output).
- [ ] Write the minimal implementation (show the actual code, not pseudocode).
- [ ] Run it and verify it PASSES (give the exact command + show the expected success output).
- [ ] Commit.

Example:
```
- [ ] Write the failing test. Create `core/tests/split_threshold_render.rs`:
  ```rust
  #[test]
  fn renders_split_threshold_placeholder_in_design_mode() {
      let mode = CollaborationMode {
          mode: ModeKind::Design,
          settings: Settings {
              developer_instructions: Some("Split at {{ split_threshold }} subsystems.".to_string()),
              ...
          },
      };
      let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
          .expect("should produce instructions");
      assert_eq!(instructions.body(), "Split at 8 subsystems.");
  }
  ```
- [ ] Run it and verify FAILS:
  ```bash
  cargo test -p ody-core split_threshold_render
  ```
  Expected output:
  ```
  thread 'split_threshold_render' panicked at '
  assertion failed: instructions.body() == "Split at 8 subsystems."
  ...
  ```
- [ ] Write the minimal implementation in `core/src/context/collaboration_mode_instructions.rs`:
  ```rust
  fn render_plan_instructions(instructions: &str, split_threshold: Option<usize>) -> String {
      let template = ody_utils_template::Template::parse(instructions).ok()?;
      let value = split_threshold.map_or_else(|| "8".to_string(), |v| v.to_string());
      template.render([("split_threshold", value.as_str())]).ok().unwrap_or_else(|| instructions.to_string())
  }
  ```
- [ ] Run it and verify PASSES:
  ```bash
  cargo test -p ody-core split_threshold_render
  ```
  Expected output:
  ```
  test split_threshold_render ... ok
  ```
- [ ] Commit:
  ```bash
  git add core/tests/split_threshold_render.rs core/src/context/collaboration_mode_instructions.rs
  git commit -m "feat: render split_threshold placeholder in Design mode"
  ```
```

**For filter/regex/matching rules:** Explicitly enumerate 2–3 inputs that MUST survive (not be filtered out) and verify that none of them are caught by the word list or regex you wrote. If a must-survive input contains a sensitive word, the constant is wrong and must be fixed before the test.

**For pure helper functions:** Light test is OK (may just be a compile check if the function is trivial).

**Never do this:**
- Collect tests into a trailing "write the tests" task.
- Use pseudocode like `// set user_id to some value`.
- Say "similar to Task N" without repeating the actual code.
- Use a must-survive input that your filter/regex would reject.

### Implementation (non-testable code — complete code + manual verification)

For UI components, configuration files, wiring/routing, and declarative markup:

- [ ] Write the complete code (show the actual implementation, not a sketch).
- [ ] Build/typecheck step (give the exact command + expected success).
- [ ] Manual-verification step (describe the exact action a user would take + expected observation).
- [ ] Commit.

Example:
```
- [ ] Write the complete code. Update `app-server/tests/suite/v2/collaboration_mode_list.rs:28-57`:
  ```rust
  #[test]
  fn list_collaboration_modes_returns_presets() {
      let expected = builtin_collaboration_mode_presets();
      let items = get_collaboration_modes();
      assert_eq!(expected, items);
      
      // Explicit assertion to prevent regression: Design must be in the list
      let modes: Vec<Option<ModeKind>> = items.iter().map(|item| item.mode).collect();
      assert!(
          modes.contains(&Some(ModeKind::Design)),
          "collaboration mode list must include Design; got {:?}",
          modes
      );
  }
  ```
- [ ] Build/typecheck:
  ```bash
  cargo test -p ody-app-server --test suite list_collaboration_modes_returns_presets
  ```
  Expected output:
  ```
  test list_collaboration_modes_returns_presets ... ok
  ```
- [ ] Manual verification: Open the app settings UI, navigate to Collaboration Mode selector, and confirm that "Design" appears in the dropdown list.
- [ ] Commit:
  ```bash
  git add app-server/tests/suite/v2/collaboration_mode_list.rs
  git commit -m "test(app-server): assert collaboration mode list includes Design"
  ```
```

### Shared-signature changes

If a task changes a type, function signature, struct field, or any API that other code in the codebase already uses:

1. **Find every caller** (use grep/Glob to search):
   ```bash
   grep -rn "function_name(" packages/ --include="*.rs"
   ```
   Include test files in your search.

2. **Update every caller** in the same task (do not defer to a later task).

3. **Whole-tree typecheck** (not a single-package build):
   ```bash
   cargo check --workspace --all-targets
   cargo test --workspace
   ```
   Ensure no downstream callers are broken.

4. **Never split shared-signature changes** across multiple tasks.

### No placeholders or pseudocode

These are plan failures:
- `TODO`/`TBD`/"implement later"
- "add appropriate error handling" without the actual error handling code
- "similar to Task N" without repeating the actual code
- References to types, functions, or files that no earlier task defines
- Author deliberation or open questions left in the body
- `// TODO: validate input` without the actual validation code

Every step must contain the real, complete content an engineer needs to execute it right away.

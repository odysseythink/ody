# E2E Verification — max_tokens truncation notice (core → app-server → TUI)

- **Date:** 2026-07-15
- **Branch:** `feat/max-tokens-truncation-notice` (4 commits on `main` @ `3c6cc25`, HEAD `4071de7`)
- **Verifier:** Task 5 implementer subagent (read-only verification; no Rust code changes)
- **Verdict:** PASS — full downstream chain confirmed by code inspection **and** by a live TUI pty run; no fallback needed. Regression failure sets on the branch are a strict subset of (in fact identical to) the `main` baseline on this machine.

Feature under test: when a Chat Completions provider ends a response with
`finish_reason: "length"`/`"max_tokens"`, core emits `EventMsg::Warning`:

```
Model hit the provider's max output token limit (finish_reason=length); the response may be incomplete.
```

(emitted at `core/src/session/turn.rs:2383-2392`, `reason @ ("length" | "max_tokens")` guard.)

---

## 1. Downstream chain — read-only confirmation

All four links exist unchanged on the branch.

### 1.1 core → app-server: `EventMsg::Warning` → `ServerNotification::Warning`

`app-server/src/bespoke_event_handling.rs:226-233`:

```rust
EventMsg::Warning(warning_event) => {
    let notification = WarningNotification {
        thread_id: Some(conversation_id.to_string()),
        message: warning_event.message,
    };
    outgoing
        .send_server_notification(ServerNotification::Warning(notification))
        .await;
}
```

### 1.2 app-server → TUI: notification dispatch

`tui/src/chatwidget/protocol.rs:150`:

```rust
ServerNotification::Warning(notification) => self.on_warning(notification.message),
```

### 1.3 TUI render: warning cell into history

`tui/src/chatwidget/turn_runtime.rs:384-398` — `on_warning` gates on
`warning_display_state.should_display(&message)` (dedupe) and then renders:

```rust
if !self.warning_display_state.should_display(&message) {
    return;
}
self.add_to_history(history_cell::new_warning_event(message));
self.request_redraw();
```

### 1.4 Replay filter: Warning is a notice

`tui/src/app/replay_filter.rs:24-32`:

```rust
pub(super) fn event_is_notice(event: &ThreadBufferedEvent) -> bool {
    matches!(
        event,
        ThreadBufferedEvent::Notification(
            ServerNotification::Warning(_)
                | ServerNotification::GuardianWarning(_)
                | ServerNotification::ConfigWarning(_)
        )
    )
}
```

Consumed at `tui/src/app/thread_routing.rs:1313-1337`: when the snapshot has a
pending interactive request (`suppress_replay_notices`), notice events are
skipped during thread-switch replay (`continue` at :1333-1335).

### Replay semantics note (carry forward)

The truncation notice is **live-only**:

- It is a `ServerNotification`, not a persisted history item — the rollout
  `sessions/.../rollout-*.jsonl` from the live run below contains the assistant
  `agent_message` but **no** warning record (verified: 0 matches for
  `finish_reason` in the rollout), so `ody resume` replay cannot re-show it.
- Within a live session, thread-switch replay additionally suppresses notice
  events (`replay_filter.rs` above) whenever the snapshot carries a pending
  interactive request.
- Persisting the notice into history (so it survives resume) would require a
  future roadmap change: emit a history item instead of (or in addition to) a
  notification.

---

## 2. Manual E2E (chat provider, full chain) — live TUI pty run SUCCEEDED

No fallback to the app-server level was needed. Harness (all under
`/tmp/ody-maxtok-verify/`, throwaway; repo and real `$ODY_HOME` untouched):

### 2.a Mock Chat Completions server

`mock_chat.py` (python3 `http.server`, `127.0.0.1:8787`). On
`POST /v1/chat/completions` returns `text/event-stream`:

```
data: {"id":"resp_1","choices":[{"delta":{"role":"assistant","content":"partial answer"}}]}
data: {"id":"resp_1","choices":[{"delta":{},"finish_reason":"length"}]}
data: {"id":"resp_1","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}
data: [DONE]
```

(same shape as `core/tests/suite/max_tokens_truncation.rs::chat_sse_with_finish_reason`,
including the usage chunk — production-realistic since the chat-wire path always
sends `stream_options.include_usage: true`). Every request is appended to
`mock_requests.log`.

### 2.b Isolated ODY_HOME

`/tmp/ody-maxtok-verify/home/config.toml` (schema taken from
`mcp-server/tests/suite/ody_tool.rs::create_config_toml` and
`core/src/config/config_tests.rs`, not guessed):

```toml
model = "mock-model"
model_provider = "mock"
approval_policy = "never"
sandbox_policy = "read-only"

[model_providers.mock]
name = "Mock chat provider"
base_url = "http://127.0.0.1:8787/v1"
wire_api = "chat"
env_key = "PATH"
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 5000

[projects."/tmp/ody-maxtok-verify/work"]
trust_level = "trusted"   # bypasses the first-run trust screen
```

The real `~/.ody-code` was never touched (`ODY_HOME` env var pointed at the
temp dir for every run).

### 2.c pty drive

`drive_tui.py` (python3 `pty`+`select`, 120x40) runs
`ODY_HOME=<tmp> RUST_LOG=trace target/debug/ody -c log_dir=<tmp>/logs`
with cwd = the temp work dir. Per the `test-tui` user skill: text written
first (`say hi`), Enter sent in a separate write 0.8 s later.

**Observed (live run, `capture_run.raw.txt`):**

- Assistant text rendered: `• partial answer`
- Warning cell rendered in the history area with the exact expected text:
  `⚠ Model hit the provider's max output token limit (finish_reason=length); the response may be incomplete.`
- Turn ended normally and the TUI exited cleanly (exit code 0).
- Mock request log: **exactly 1 `POST /v1/chat/completions`** — no
  auto-follow-up re-prompt (`needs_follow_up` stayed false).

### 2.e Replay check (live)

Second pty run: `ody resume --last` against the same isolated home
(`capture_resume.raw.txt`):

- `partial answer` re-rendered from the rollout (history replay works).
- The warning text appears **0 times** — confirms the live-only semantics of
  §1.4 (notification never persisted; nothing to replay).
- Mock request count unchanged (resume issued no new model request).

---

## 3. Regression (branch vs `main` baseline @ `3c6cc25`)

Commands exactly as specified (`cargo test`, not nextest, to match the task's
acceptance criteria). Baseline ran in a detached worktree
(`.worktrees/main-baseline`, removed after).

| Suite | Branch | `main` baseline | Verdict |
|---|---|---|---|
| `cargo test -p ody-api --tests` | **140 passed, 0 failed** (5 binaries: 125+7+1+6+1) | n/a (green required) | ✅ |
| `cargo test -p ody-model-provider --tests` | **56 passed, 0 failed** | n/a (56 expected) | ✅ |
| `cargo test -p ody-core --lib` | 2152 passed, **4 failed**, 4 ignored | 2147 passed, **5 failed**, 4 ignored | ✅ branch failures ⊊ main |
| `cargo test -p ody-core --test all` | 131 passed, **675 failed**, then env hang (killed) | 129 passed, **675 failed**, then env hang (killed) | ✅ failure sets **byte-identical** |

### `ody-core --lib` detail

Branch failure set (stable across 2 runs):
`guardian::tests::guardian_review_exhausts_three_failures_with_one_terminal_event`,
`guardian::tests::guardian_review_surfaces_responses_api_errors_in_rejection_reason`,
`session::tests::fork_startup_context_then_first_turn_diff_snapshot`,
`tools::tool_dispatch_trace::tests::missing_code_mode_wait_traces_only_the_wait_tool_call`.

All 4 reproduce identically on `main` (same assertion diffs; the guardian ones
fail with `request_log.requests().len() == 0`, the code-mode one with
"compiled without the `v8` feature" — the documented default-off gate).
`main` additionally fails
`session::tests::session_configured_reports_permission_profile_for_external_sandbox`,
which **passes** on the branch — i.e. the branch fixes nothing and breaks
nothing; these are all pre-existing environmental failures on this machine.
(The task brief anticipated only the fork-snapshot one; the fuller baseline
comparison supersedes that estimate.)

### `ody-core --test all` detail

Both runs mass-fail 675 tests deterministically (identical failure lists
across 2 branch runs and 1 isolated main run — verified with `diff`, empty
delta) and then hang at 0% CPU inside the same known-slow tests
(`suite::compact::manual_compact_emits_context_compaction_items`,
`suite::compact_remote::remote_manual_compact_emits_context_compaction_items`,
`suite::agent_websocket::websocket_v2_first_turn_*`,
`suite::agent_websocket::websocket_v2_next_turn_*`; identified via `sample`
on the stuck test binary). Both were SIGTERM'd after >10 min without log
progress, per the task's escape hatch.

Representative environmental root causes (identical panics on branch and main;
both spot-checked):

- `suite::client::azure_responses_request_includes_store_and_reasoning_ids`:
  mock-server validator panic `function_call_output without matching call in
  input: custom-tool-call-id` (`core/tests/common/responses.rs:1645`) — byte
  identical failure on `main`.
- `suite::abort_tasks::interrupt_tool_records_history_entries`:
  `timeout waiting for event` (`core/tests/common/lib.rs:347`).

One baseline pitfall worth recording: the first main run reported **714**
failures because the worktree had no `target/debug/ody` binary, so every
`assert_cmd::cargo_bin("ody")` subprocess test failed fast
(`CARGO_BIN_EXE_ody is unset`). After `cargo build -p ody-cli` in the
worktree, main converged to the same 675/675 identical-set result — and only
then did it exhibit the same compact/websocket hang as the branch.

The 2 new tests pass and are not in the failure set:

```
test suite::max_tokens_truncation::max_tokens_truncation_emits_warning_and_ends_turn ... ok
test suite::max_tokens_truncation::normal_stop_finish_reason_emits_no_warning ... ok
```

Branch `+2` passing vs main (131 vs 129) in `--test all` is exactly these two
new tests.

Per repo AGENTS.md, no full-workspace `cargo test` was run (CI's job).

---

## 4. Known limitations / review notes to carry forward

1. **Empty-completion truncation edge case.** A response truncated with ZERO
   visible output (`finish_reason: "length"` but no text/reasoning/tool items)
   still surfaces as the retryable `empty_completion` error at
   `model-provider/src/adapters/common.rs:107-111` — the check
   (`end_turn != Some(false) && !state.saw_output`) fires before the MaxTokens
   warning path is reached. Intentional for now (request/turn-level retry may
   recover); revisit if it bites in production.
2. **Behavior alignment from the Task 3 amendment.** Paused/error turns WITH
   usage now yield `end_turn: Some(false)` exactly like their no-usage
   counterparts (previously usage presence flipped them to `Some(true)` because
   the Usage-derived `Completed` short-circuited `Finish`). Accepted by the
   user as a latent-inconsistency fix.
3. **Legacy shim debt.** `model-provider/src/adapters/core.rs:23`
   `to_response_event` (test-only, no production callers) still maps
   `ChatEvent::Usage` to its own terminal `Completed`, and its doc comment
   (predating this branch) mentions usage folding that now lives in
   `core::client::map_chat_stream`. Cosmetic debt; shim slated for removal.
4. **Task 3 amendment proven load-bearing by this E2E.** The usage
   buffering/folding in `map_chat_stream` was a user-approved plan amendment
   discovered during code review — the original plan's pipeline would never
   have fired the warning when providers report usage. The live run here used
   a usage chunk and still produced the warning, confirming the amendment
   works end-to-end (the chat-wire path always sends
   `stream_options.include_usage: true`).

## 5. Artifacts

Throwaway harness under `/tmp/ody-maxtok-verify/` (mock server, isolated home,
pty driver, raw + ANSI-stripped captures `capture_run.raw[.txt]`,
`capture_resume.raw[.txt]`, regression logs `branch_test_all2.log`,
`main_test_all2.log`, `baseline_lib_full.log`, failure-set diffs). Not
committed; reproducible from §2 on demand.

---
name: systematic-debugging
description: Use when encountering any bug, test failure, or unexpected behavior, before proposing fixes
namespace: core
---

# Systematic Debugging

## Overview

Random fixes waste time and create new bugs. Quick patches mask underlying issues.

**Core principle:** ALWAYS find root cause before attempting fixes. Symptom fixes are failure.

**Violating the letter of this process is violating the spirit of debugging.**

## The Iron Law

```
NO FIXES WITHOUT ROOT CAUSE INVESTIGATION FIRST
```

If you haven't completed Phase 1, you cannot propose fixes.

## The Evidence Rule

Before proposing a fix or declaring a root cause, you MUST cite evidence supporting that conclusion.

Acceptable evidence includes:
- Code snippets (the actual lines involved)
- Log output or stack traces
- Database records or query results
- Configuration files or environment variables
- Test output or reproduction steps
- Network traces / API responses

**Hypotheses are NOT conclusions.** If you haven't verified it yet, state it as: "Hypothesis: X. Evidence needed: Y. Next verification step: Z."

**Do not present speculation as fact.** Phrases like "probably because", "maybe it's", or "I suspect" are warning signs that you need to gather evidence first.

If evidence is temporarily unavailable (e.g., external system, intermittent failure), document the gap explicitly and treat the claim as an unverified hypothesis.

## The No-Happy-Path Rule

Debugging is not about finding a story that makes the failure sound reasonable.
It is about finding the truth, even when the truth is messy, inconvenient, or
points to a mistake you made.

Operate as a **rigorous, skeptical engineering partner**. Do not act as a
helpful assistant trying to make the user feel better. Your job is to make the
failure impossible to ignore, not to make it easier to accept.

Before accepting any hypothesis or fix, you MUST be able to answer:

1. **What would prove this wrong?**
   Define the evidence that would falsify your current hypothesis.

2. **Have we looked for that evidence and ruled it out?**
   Optimism is not a substitute for observation.

3. **What are we assuming that we have not checked?**
   List every assumption, no matter how obvious it seems.

4. **Is there a simpler explanation that fits all observations?**
   The simplest explanation that accounts for everything is the one to beat.

5. **What if the failure is not where it looks?**
   Consider that the error might be in a different layer, component, or earlier
   step than the obvious one.

If you cannot answer these questions, you are on a happy path. **STOP. Return to
Phase 1.**

## The Minimal-Experiment Rule

For a **critical or high-risk hypothesis** — one where guessing wrong is expensive, or where the behavior can only be known by observing it — do not argue your way to a conclusion. Write the **smallest possible experiment** (a throwaway reproduction, a one-off probe script, a targeted diagnostic) that would confirm or falsify it, run it, and let the result decide. This is the concrete form of the Evidence Rule; Phase 1 (step 7) and Phase 3 (Test Minimally) are where it lives.

State, before running: the **one question** the experiment answers, its **pass/fail criterion**, and explicitly **what it does not cover** — an experiment that quietly simplifies away the real conditions produces false confidence, which is worse than none.

### Prefer real data — but handle it safely

A reproduction is only as trustworthy as its inputs, so **prefer real data over mocks** when the bug depends on the shape, scale, or messiness of real data. Real data sources are the highest-risk part of debugging — apply every rule below:

- **Snapshots over live connections.** Usually you need the *shape* of the data, not a live link. Prefer a **desensitized/anonymized sample** (a dump, a CSV, a batch of scrubbed records) copied into an isolated scratch location. Connect to a live store only when the bug itself is about live behavior (connection pooling, real latency, online semantics).
- **Read-only, non-production by default.** If you need a connection string or config path, use a **read-only credential against a non-production environment** (dev / staging / shadow). Never run a reproduction that **writes** against a real store, and never run a destructive repro against production.
- **Credentials via environment variables only.** Read them from **env vars / config files**; never hardcode them into a repro script and **never paste them into this conversation, into logs, or into a bug report**. Keep any `.env`/config file out of version control. Note that some libraries print the full connection string (with password) in stack traces — scrub before surfacing output.
- **Connection tiering.** Read-only + non-production + desensitized → fine to run. Anything touching **production, writes, or sensitive data** → don't; if it is truly unavoidable, ask your human partner first and state the exact scope.
- **Run repros in a throwaway, isolated location** (a scratch dir, a temp file, a disposable script), not inside the code you are shipping. Env vars stop "hardcoded → committed" credential leaks, but not command-history, process-environment, conversation, log, or data-exfiltration leaks — which is why read-only + non-production + snapshot-first still stands.

Feed the result back as evidence: **conclusion + the data that shows it + the boundary of what it proves.** Then discard the experiment's code — it carries every shortcut you took for speed and must not be lifted into the fix.


## When to Use

Use for ANY technical issue:
- Test failures
- Bugs in production
- Unexpected behavior
- Performance problems
- Build failures
- Integration issues

**Use this ESPECIALLY when:**
- Under time pressure (emergencies make guessing tempting)
- "Just one quick fix" seems obvious
- You've already tried multiple fixes
- Previous fix didn't work
- You don't fully understand the issue

**Don't skip when:**
- Issue seems simple (simple bugs have root causes too)
- You're in a hurry (rushing guarantees rework)
- Manager wants it fixed NOW (systematic is faster than thrashing)

## The Four Phases

You MUST complete each phase before proceeding to the next.

### Phase 1: Root Cause Investigation

**BEFORE attempting ANY fix:**

1. **Clarify Ambiguous Language Before Investigating**

   Before you read error messages or gather evidence, check whether the user's description contains a term that is ambiguous **in this specific debugging context**. A term is worth clarifying only if multiple meanings would lead to *different root-cause investigation paths*. If the meaning is already obvious from logs, stack traces, or code context, do not ask.

   **When to clarify (all must be true):**
   - The user relies on a multi-meaning term (e.g., "报错", "挂了", "不行了", "卡住", "失败", "慢", "没反应").
   - The term could refer to at least two different failure modes in this codebase.
   - Choosing the wrong meaning would send the investigation down a materially different path, AND the user has not already provided enough context (logs, code snippets, examples, session paths) to disambiguate it.

   **How to clarify:**
   1. Identify the ambiguous term(s).
   2. Based on the current code, recent changes, and any available logs, list the most likely meanings as concrete options.
   3. For each option, state what evidence you would look for first if that option were correct.
   4. Ask the user to confirm or correct, and always include an escape hatch: "都不是，我的意思是...".
   5. **If evidence would disambiguate, fetch it.** When the user has already provided a log, session path, or code example, start reading it immediately while asking the clarification question. Do not wait for an answer before gathering evidence.

   **Example:**
   > 你提到的"报错"可能指几种不同情况：
   > 1. 编译错误：我会先看 `cargo check` 输出和最近的代码改动。
   > 2. 运行时 panic：我会先找 panic 信息和调用栈。
   > 3. 测试失败：我会先运行失败的测试并看断言输出。
   > 4. 工具/脚本返回非零退出码：我会先看 stderr 和 stdout。
   >
   > 你遇到的是哪一种？如果不是以上，请直接描述。我会同时根据你提供的信息先读取相关证据。

   **Do not:**
   - Clarify terms already disambiguated by context.
   - Ask about multiple unrelated terms at once.
   - Present options without linking them to concrete evidence paths.
   - Stop investigating just to wait for clarification when the user has already provided concrete evidence (logs, code snippets, examples, session paths).
   - Output tool calls as XML tags inside assistant message text (e.g., `<function=shell_command>`, `<tool=...>`). These are not executed. If you intend to use a tool, emit a real `function_call` / `tool_call`.

2. **Read Error Messages Carefully**
   - Don't skip past errors or warnings
   - They often contain the exact solution
   - Read stack traces completely
   - Note line numbers, file paths, error codes

3. **Reproduce Consistently**
   - Can you trigger it reliably?
   - What are the exact steps?
   - Does it happen every time?
   - If not reproducible → gather more data, don't guess

4. **Check Recent Changes**
   - What changed that could cause this?
   - Git diff, recent commits
   - New dependencies, config changes
   - Environmental differences

5. **Gather Evidence in Multi-Component Systems**

   **WHEN system has multiple components (CI → build → signing, API → service → database):**

   **BEFORE proposing fixes, add diagnostic instrumentation:**
   ```
   For EACH component boundary:
     - Log what data enters component
     - Log what data exits component
     - Verify environment/config propagation
     - Check state at each layer

   Run once to gather evidence showing WHERE it breaks
   THEN analyze evidence to identify failing component
   THEN investigate that specific component
   ```

   **Example (multi-layer system):**
   ```bash
   # Layer 1: Workflow
   echo "=== Secrets available in workflow: ==="
   test -n "$IDENTITY" && echo "IDENTITY: SET" || echo "IDENTITY: UNSET"

   # Layer 2: Build script
   echo "=== Env vars in build script: ==="
   env | grep IDENTITY || echo "IDENTITY not in environment"

   # Layer 3: Signing script
   echo "=== Keychain state: ==="
   security list-keychains
   security find-identity -v

   # Layer 4: Actual signing
   codesign --sign "$IDENTITY" --verbose=4 "$APP"
   ```

   **This reveals:** Which layer fails (secrets → workflow ✓, workflow → build ✗)

6. **Trace Data Flow**

   **WHEN error is deep in call stack:**

   See `root-cause-tracing.md` in this directory for the complete backward tracing technique.

   **Quick version:**
   - Where does bad value originate?
   - What called this with bad value?
   - Keep tracing up until you find the source
   - Fix at source, not at symptom

7. **If You Are Still Stuck: Add Targeted Diagnostics and Re-run**

   If you have tried the above and still cannot form a testable hypothesis, **do not continue guessing.** The next step is to make the invisible visible.

   **When to use this step:**
   - You have a clear suspicion but no evidence to confirm or disprove it.
   - The failure is intermittent or environment-dependent.
   - A key variable's value, state transition, or boundary crossing is not visible in existing logs or code.

   **How to add diagnostics:**
   - Pick the narrowest possible observation point: the function, state transition, or component boundary where your hypothesis is most likely to break.
   - Log what goes in, what comes out, and any relevant intermediate state.
   - Keep the diagnostic change minimal and reversible.
   - Each log must answer a specific question. If you cannot state that question in one sentence, you are not ready to add the log.

   **Before running, decide who can run it:**
   - If you can safely add and run the diagnostic in the current environment, do so.
   - If the environment is read-only, the reproduction is expensive, or you cannot run the test yourself, ask the user to add the diagnostic, re-run, and provide the resulting log output.

   **After re-running:** Return to evidence analysis. Do not proceed to Phase 2 until you have a concrete, evidence-backed hypothesis.

### Phase 2: Pattern Analysis

**Find the pattern before fixing:**

1. **Find Working Examples**
   - Locate similar working code in same codebase
   - What works that's similar to what's broken?

2. **Compare Against References**
   - If implementing pattern, read reference implementation COMPLETELY
   - Don't skim - read every line
   - Understand the pattern fully before applying

3. **Identify Differences**
   - What's different between working and broken?
   - List every difference, however small
   - Don't assume "that can't matter"

4. **Understand Dependencies**
   - What other components does this need?
   - What settings, config, environment?
   - What assumptions does it make?

### Phase 3: Hypothesis and Testing

**Scientific method:**

1. **Form Single Hypothesis**
   - State clearly: "I think X is the root cause because Y"
   - Write it down
   - Be specific, not vague

2. **Test Minimally**
   - Make the SMALLEST possible change to test hypothesis
   - One variable at a time
   - Don't fix multiple things at once

3. **Verify Before Continuing**
   - Did it work? Yes → Phase 4
   - Didn't work? Form NEW hypothesis
   - DON'T add more fixes on top

4. **When You Don't Know**
   - Say "I don't understand X"
   - Don't pretend to know
   - Ask for help
   - Research more

### Phase 4: Implementation

**Fix the root cause, not the symptom:**

1. **Create Failing Test Case**
   - Simplest possible reproduction
   - Automated test if possible
   - One-off test script if no framework
   - MUST have before fixing
   - Use the `gpowers:test-driven-development` skill for writing proper failing tests

2. **Implement Single Fix**
   - Address the root cause identified
   - ONE change at a time
   - No "while I'm here" improvements
   - No bundled refactoring

3. **Verify Fix with Controlled Comparison**

   Verification must be repeatable, not a single lucky run:
   
   - **Establish baseline first:** reproduce the failure before the fix.
     - Failing test, bad log, error state — capture it.
   - **Apply the fix and re-run the same input / conditions.**
     - Confirm the failure disappears.
   - **For intermittent, multi-factor, or environmental issues, use a control:**
     - Keep an unmodified reference environment or version.
     - Confirm the failure still happens there while the fixed version passes.
     - This rules out "it just stopped happening on its own".
   - **Check for regressions:** no other tests broken?
   - **Only when the controlled comparison passes can you call the fix successful.**

4. **If Fix Doesn't Work**
   - STOP
   - Count: How many fixes have you tried?
   - If < 3: Return to Phase 1, re-analyze with new information
   - **If ≥ 3: STOP and question the architecture (step 5 below)**
   - DON'T attempt Fix #4 without architectural discussion

5. **If 3+ Fixes Failed: Question Architecture**

   **Pattern indicating architectural problem:**
   - Each fix reveals new shared state/coupling/problem in different place
   - Fixes require "massive refactoring" to implement
   - Each fix creates new symptoms elsewhere

   **STOP and question fundamentals:**
   - Is this pattern fundamentally sound?
   - Are we "sticking with it through sheer inertia"?
   - Should we refactor architecture vs. continue fixing symptoms?

   **Discuss with your human partner before attempting more fixes**

   This is NOT a failed hypothesis - this is a wrong architecture.

## Red Flags - STOP and Follow Process

If you catch yourself thinking:
- "Quick fix for now, investigate later"
- "Just try changing X and see if it works"
- "Add multiple changes, run tests"
- "Skip the test, I'll manually verify"
- "It's probably X, let me fix that"
- "I don't fully understand but this might work"
- "Pattern says X but I'll adapt it differently"
- "Here are the main problems: [lists fixes without investigation]"
- Proposing solutions before tracing data flow
- **"One more fix attempt" (when already tried 2+)**
- **Each fix reveals new problem in different place**

**ALL of these mean: STOP. Return to Phase 1.**

**If 3+ fixes failed:** Question the architecture (see Phase 4.5)

## your human partner's Signals You're Doing It Wrong

**Watch for these redirections:**
- "Is that not happening?" - You assumed without verifying
- "Will it show us...?" - You should have added evidence gathering
- "Stop guessing" - You're proposing fixes without understanding
- "Ultrathink this" - Question fundamentals, not just symptoms
- "We're stuck?" (frustrated) - Your approach isn't working

**When you see these:** STOP. Return to Phase 1.

## Common Rationalizations

| Excuse | Reality |
|--------|---------|
| "Issue is simple, don't need process" | Simple issues have root causes too. Process is fast for simple bugs. |
| "Emergency, no time for process" | Systematic debugging is FASTER than guess-and-check thrashing. |
| "Just try this first, then investigate" | First fix sets the pattern. Do it right from the start. |
| "I'll write test after confirming fix works" | Untested fixes don't stick. Test first proves it. |
| "Multiple fixes at once saves time" | Can't isolate what worked. Causes new bugs. |
| "Reference too long, I'll adapt the pattern" | Partial understanding guarantees bugs. Read it completely. |
| "I see the problem, let me fix it" | Seeing symptoms ≠ understanding root cause. |
| "One more fix attempt" (after 2+ failures) | 3+ failures = architectural problem. Question pattern, don't fix again. |
| "I ran it once and it worked" | One successful run doesn't rule out luck, timing, or environment. Use a controlled, repeatable comparison. |

## Quick Reference

| Phase | Key Activities | Success Criteria |
|-------|---------------|------------------|
| **1. Root Cause** | Read errors, reproduce, check changes, gather evidence | Understand WHAT and WHY |
| **2. Pattern** | Find working examples, compare | Identify differences |
| **3. Hypothesis** | Form theory, test minimally | Confirmed or new hypothesis |
| **4. Implementation** | Create test, fix, verify with controlled comparison | Bug resolved, tests pass, comparison confirms fix |

## When Process Reveals "No Root Cause"

If systematic investigation reveals issue is truly environmental, timing-dependent, or external:

1. You've completed the process
2. Document what you investigated
3. Implement appropriate handling (retry, timeout, error message)
4. Add monitoring/logging for future investigation

**But:** 95% of "no root cause" cases are incomplete investigation.

## Supporting Techniques

These techniques are part of systematic debugging and available in this directory:

- **`root-cause-tracing.md`** - Trace bugs backward through call stack to find original trigger
- **`defense-in-depth.md`** - Add validation at multiple layers after finding root cause
- **`condition-based-waiting.md`** - Replace arbitrary timeouts with condition polling

**Related skills:**
- **gpowers:test-driven-development** - For creating failing test case (Phase 4, Step 1)
- **gpowers:verification-before-completion** - Verify fix worked before claiming success

## Real-World Impact

From debugging sessions:
- Systematic approach: 15-30 minutes to fix
- Random fixes approach: 2-3 hours of thrashing
- First-time fix rate: 95% vs 40%
- New bugs introduced: Near zero vs common

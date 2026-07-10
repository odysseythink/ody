---
name: executing-plans
description: Use when you have a written implementation plan to execute in a separate session with review checkpoints
namespace: core
upstream: superpowers@v5.1.0
---

# Executing Plans

## Overview

Load plan, review critically, execute all tasks, report when complete.

**Announce at start:** "I'm using the executing-plans skill to implement this plan."

**Note:** Tell your human partner that Superpowers works much better with access to subagents. The quality of its work will be significantly higher if run on a platform with subagent support (such as Claude Code or Codex). If subagents are available, use gpowers:subagent-driven-development instead of this skill.

## The Process

### Step 1: Load and Review Plan
1. Read plan file
2. Review critically - identify any questions or concerns about the plan
3. If concerns: Raise them with your human partner before starting
4. If no concerns: Create TodoWrite and proceed

### Step 2: Execute Tasks

For each task:
1. Mark as in_progress
2. Follow each step exactly (plan has bite-sized steps)
3. Run verifications as specified
4. Mark as completed

### Step 3: Complete Development

**Precondition:** All tasks in the plan are marked `completed` with passing verification commands.

When Step 3 is reached:
1. **Announce transition:** "Tasks complete. Moving to finishing-a-development-branch skill for final handoff."
2. **Check plan status:** Verify every task shows `completed` (not `in_progress` or `pending`).
3. **Call finishing-a-development-branch:** Use the referenced skill to present options (merge/PR/cleanup) and execute the user's choice.
   - **Do NOT:** Skip this step or assume work is done after verification passes.
   - **Do NOT:** Send empty responses. Always announce which skill you're transitioning to.

If a task remains `pending` or `in_progress`, return to Step 2 and complete it first.

## When to Stop and Ask for Help

**STOP executing immediately when:**
- Hit a blocker (missing dependency, test fails, instruction unclear)
- Plan has critical gaps preventing starting
- You don't understand an instruction
- Verification fails repeatedly

**Ask for clarification rather than guessing.**

### Blocker Handling Protocol

When you encounter a blocker:

1. **Assess severity:** Is this an obvious local fix (e.g., `ModeKind` match arm, typo in instructions, straightforward compilation error) vs. a true architectural issue?
   - **Obvious local fix:** Apply the fix directly and continue. Do NOT STOP for trivial fixes.
   - **True blocker:** Missing dependency, fundamental design conflict, unclear scope — STOP.

2. **When you STOP:** Always emit a clear message with:
   - **What stopped you:** Exact error message or blocker description
   - **Why you stopped:** Reference the specific STOP condition from above
   - **What you need:** Explicit question for the user (e.g., "Should I patch this match arm, or is Design mode not ready?")

3. **Do NOT send empty responses.** If uncertain whether to STOP or continue, output the uncertainty verbatim:
   ```
   Encountered [blocker]. This appears to be [local fix / true blocker]. 
   Proceeding [or: Stopping — need your input on:] [specific question].
   ```

## When to Revisit Earlier Steps

**Return to Review (Step 1) when:**
- Partner updates the plan based on your feedback
- Fundamental approach needs rethinking

**Don't force through blockers** - stop and ask.

## Remember
- Review plan critically first
- Follow plan steps exactly
- Don't skip verifications
- Reference skills when plan says to
- Stop when blocked, don't guess
- Never start implementation on main/master branch without explicit user consent

## Integration

**Required workflow skills:**
- **gpowers:using-git-worktrees** - Ensures isolated workspace (creates one or verifies existing)
- **gpowers:writing-plans** - Creates the plan this skill executes
- **gpowers:finishing-a-development-branch** - Complete development after all tasks

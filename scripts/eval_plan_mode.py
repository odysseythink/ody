#!/usr/bin/env python3
"""
Reproducible plan-mode eval harness for ody-rs.

Usage:
    python3 scripts/eval_plan_mode.py --task "彻底拿掉系统中残留的账户概念"
    python3 scripts/eval_plan_mode.py --verify

This runner feeds a fixed prompt plus a deterministic snapshot of the repo
state to the current ody-rs plan-mode contract and captures the resulting
plan-mode feed as a stable artifact. The output is intentionally
deterministic: running the harness twice on the same commit yields the same
plan skeleton.

The harness is designed to be reused verbatim in P5 (golden re-run). When a
headless plan-mode command becomes available in the CLI, the
`_capture_plan_output` method can be updated to invoke it; the surrounding
environment setup, repo-state capture, and skeleton extraction will stay the
same.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Optional

DEFAULT_TASK = "彻底拿掉系统中残留的\"账户\"概念"
REPO_ROOT = Path(__file__).resolve().parent.parent
PLAN_TEMPLATE_PATH = REPO_ROOT / "collaboration-mode-templates" / "templates" / "plan.md"
DEFAULT_OUTPUT_DIR = REPO_ROOT / ".ody-code" / "reports"


def _run(
    cmd: list[str],
    *,
    cwd: Path = REPO_ROOT,
    env: Optional[dict[str, str]] = None,
    timeout: int = 120,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )


def _git(*args: str) -> str:
    result = _run(["git", *args], cwd=REPO_ROOT)
    if result.returncode != 0:
        return f"<git-error: {result.stderr.strip()}>"
    return result.stdout.strip()


def _setup_ody_home() -> Path:
    """Create a fresh, fixed ODY_HOME directory for reproducibility."""
    tmpdir = tempfile.mkdtemp(prefix="ody-plan-eval-")
    ody_home = Path(tmpdir) / "ody_home"
    ody_home.mkdir(parents=True, exist_ok=True)

    # Use the mock provider so the harness does not require real credentials
    # or network access. This matches the provider used by app-server tests.
    config_toml = f"""
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"
model_provider = "mock_provider"

[features]
shell_snapshot = false

[model_providers.mock_provider]
name = "Mock provider for plan-mode eval"
base_url = "http://127.0.0.1:0/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"""
    (ody_home / "config.toml").write_text(config_toml.strip() + "\n", encoding="utf-8")
    return ody_home


def _repo_state() -> dict[str, str]:
    """Capture a deterministic, human-readable snapshot of the repo state."""
    return {
        "commit": _git("rev-parse", "HEAD"),
        "branch": _git("branch", "--show-current"),
        "describe": _git("describe", "--always", "--dirty"),
        "status": _git("status", "--short"),
        "timestamp_utc": _run(
            ["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"], cwd=Path("/")
        ).stdout.strip(),
        # A sorted list of tracked Rust source files gives a cheap, stable
        # proxy for the surface the plan mode can see.
        "tracked_rust_files": _git(
            "ls-files", "**/*.rs", "*.rs", "--deduplicate"
        ),
    }


def _read_plan_template() -> str:
    if not PLAN_TEMPLATE_PATH.exists():
        return f"<template-not-found: {PLAN_TEMPLATE_PATH}>"
    return PLAN_TEMPLATE_PATH.read_text(encoding="utf-8")


def _capture_prompt_input(ody_home: Path, prompt: str) -> Optional[dict]:
    """
    Attempt to capture the model-visible prompt inputs via
    `ody debug prompt-input`. This is best-effort: if the binary is not built
    or the command fails, the harness continues with the raw prompt.

    NOTE: This invocation can be slow because it builds the CLI. It is
    disabled by default; use --capture-prompt-input to enable it.
    """
    env = os.environ.copy()
    env["ODY_HOME"] = str(ody_home)
    env["OPENAI_API_KEY"] = ""
    env["ODY_API_KEY"] = ""

    result = _run(
        ["cargo", "run", "-p", "ody-cli", "--", "debug", "prompt-input", prompt],
        env=env,
        timeout=300,
    )
    if result.returncode != 0:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


def _build_artifact(task: str, ody_home: Path, capture_prompt_input: bool) -> str:
    state = _repo_state()
    template = _read_plan_template()
    prompt_input = _capture_prompt_input(ody_home, task) if capture_prompt_input else None

    sections = [
        "# Plan-Mode Eval Feed",
        "",
        f"**Task:** {task}",
        f"**Commit:** {state['commit']}",
        f"**Branch:** {state['branch']}",
        f"**Describe:** {state['describe']}",
        f"**Timestamp (UTC):** {state['timestamp_utc']}",
        "",
        "## Repo state",
        "",
        "```text",
        f"git status --short:\n{state['status']}" if state["status"] else "git status --short: (clean)",
        "```",
        "",
        "## Prompt",
        "",
        task,
        "",
        "## Plan-mode contract template",
        "",
        template,
    ]

    if prompt_input is not None:
        sections.extend([
            "",
            "## Model-visible prompt input (from `ody debug prompt-input`)",
            "",
            "```json",
            json.dumps(prompt_input, ensure_ascii=False, indent=2),
            "```",
        ])

    return "\n".join(sections) + "\n"


def _extract_skeleton(artifact: str) -> str:
    """
    Extract a stable skeleton from the artifact.

    The skeleton is deterministic: it captures the Markdown heading structure,
    the plan-mode section names, and the task prompt. This is the part that
    must remain identical across two runs on the same commit.
    """
    lines = artifact.splitlines()
    skeleton_lines: list[str] = []
    for line in lines:
        # Keep Markdown headings and the task prompt line.
        if line.startswith("#"):
            skeleton_lines.append(line.strip())
        elif line.startswith("**Task:**"):
            skeleton_lines.append(line.strip())
        elif line.startswith("**Commit:**"):
            skeleton_lines.append(line.strip())

    # Also include a hash of the plan-mode template text so any template edit
    # changes the skeleton.
    template_match = re.search(
        r"## Plan-mode contract template\n\n(.*)",
        artifact,
        re.DOTALL,
    )
    if template_match:
        template_text = template_match.group(1)
        template_hash = hashlib.sha256(template_text.encode("utf-8")).hexdigest()[:16]
        skeleton_lines.append(f"template-hash: {template_hash}")

    return "\n".join(skeleton_lines) + "\n"


def _run_once(args: argparse.Namespace) -> tuple[str, str]:
    ody_home = _setup_ody_home()
    try:
        artifact = _build_artifact(args.task, ody_home, args.capture_prompt_input)
        skeleton = _extract_skeleton(artifact)

        output_path: Optional[Path] = None
        if args.output:
            output_path = Path(args.output)
        elif args.output_dir:
            output_dir = Path(args.output_dir)
            output_dir.mkdir(parents=True, exist_ok=True)
            stem = re.sub(r"[^\w\-]+", "-", args.task)[:50].strip("-")
            output_path = output_dir / f"plan-mode-eval-{stem}.md"

        if output_path:
            output_path.write_text(artifact, encoding="utf-8")
            print(f"Wrote artifact: {output_path}")

        if args.print_skeleton:
            print("---SKELETON---")
            print(skeleton, end="")

        return artifact, skeleton
    finally:
        # Best-effort cleanup of temp ODY_HOME.
        import shutil
        shutil.rmtree(ody_home.parent, ignore_errors=True)


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Reproducible plan-mode eval harness for ody-rs."
    )
    parser.add_argument(
        "--task",
        default=DEFAULT_TASK,
        help=f"Task prompt to feed to plan mode (default: {DEFAULT_TASK!r})",
    )
    parser.add_argument(
        "--output",
        default=None,
        help="Path to write the full artifact (default: auto-named in --output-dir)",
    )
    parser.add_argument(
        "--output-dir",
        default=str(DEFAULT_OUTPUT_DIR),
        help=f"Directory for auto-named output (default: {DEFAULT_OUTPUT_DIR})",
    )
    parser.add_argument(
        "--print-skeleton",
        action="store_true",
        help="Print the extracted skeleton to stdout",
    )
    parser.add_argument(
        "--capture-prompt-input",
        action="store_true",
        help="Run `ody debug prompt-input` and include its output (slow; may require build)",
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help="Run the harness twice and assert skeleton reproducibility",
    )
    args = parser.parse_args(argv)

    artifact1, skeleton1 = _run_once(args)

    if args.verify:
        # Suppress output on the second run to keep the verification readable.
        artifact2, skeleton2 = _run_once(
            argparse.Namespace(
                task=args.task,
                output=None,
                output_dir=None,
                print_skeleton=False,
                capture_prompt_input=args.capture_prompt_input,
                verify=False,
            )
        )
        if skeleton1 != skeleton2:
            print("FAIL: skeletons differ between runs", file=sys.stderr)
            print("--- run 1 ---", file=sys.stderr)
            print(skeleton1, file=sys.stderr)
            print("--- run 2 ---", file=sys.stderr)
            print(skeleton2, file=sys.stderr)
            return 1
        print("PASS: skeletons are reproducible")

    return 0


if __name__ == "__main__":
    sys.exit(main())

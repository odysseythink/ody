#!/usr/bin/env python3
"""Add `capabilities: ModelCapabilities::default(),` to explicit
`ModelInfo { ... }` struct literals that do not already have capabilities
or use struct-update syntax (`..`)."""

import re
import sys
from pathlib import Path


def update_file(path: Path) -> int:
    raw = path.read_text(encoding="utf-8")
    # Replace CRLF with LF for easier processing, restore later if needed.
    had_crlf = "\r\n" in raw
    text = raw.replace("\r\n", "\n")

    pattern = re.compile(r"\bModelInfo\s*\{")
    changed = 0
    pos = 0
    while True:
        m = pattern.search(text, pos)
        if not m:
            break

        # Heuristic: skip function return type `-> ModelInfo {`.
        # Look at the line containing the match.
        line_start = text.rfind("\n", 0, m.start()) + 1
        line = text[line_start:m.end()]
        if "->" in line:
            pos = m.end()
            continue

        # Also skip if preceded by `->` on the previous line (multiline return type).
        prev_line_start = text.rfind("\n", 0, line_start - 1) + 1 if line_start > 0 else 0
        prev_line = text[prev_line_start:line_start - 1]
        if "->" in prev_line and "{" not in prev_line:
            pos = m.end()
            continue

        # Find the matching closing brace for this struct literal.
        brace_start = m.end() - 1  # position of '{'
        brace_count = 0
        i = brace_start
        found_open = False
        while i < len(text):
            ch = text[i]
            if ch == "{":
                brace_count += 1
                found_open = True
            elif ch == "}":
                brace_count -= 1
                if found_open and brace_count == 0:
                    break
            i += 1
        if not found_open or brace_count != 0:
            pos = m.end()
            continue

        block = text[m.start():i + 1]
        # Skip if already has capabilities field or uses struct update syntax.
        if re.search(r"\bcapabilities\s*:", block) or ".." in block:
            pos = i + 1
            continue

        # Insert capabilities before the closing '}'.
        insert = "        capabilities: ModelCapabilities::default(),\n"
        # Determine indentation of closing brace.
        close_line_start = text.rfind("\n", 0, i) + 1
        close_indent = text[close_line_start:i]
        inner_indent = close_indent + "    "
        insert = inner_indent + "capabilities: ModelCapabilities::default(),\n"

        text = text[:i] + insert + text[i:]
        changed += 1
        pos = i + len(insert) + 1

    if changed:
        new_raw = text.replace("\n", "\r\n") if had_crlf else text
        path.write_text(new_raw, encoding="utf-8")
    return changed


def main() -> int:
    root = Path(".")
    total = 0
    for path in root.rglob("*.rs"):
        n = update_file(path)
        if n:
            print(f"{path}: updated {n} literal(s)")
            total += n
    print(f"Total literals updated: {total}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

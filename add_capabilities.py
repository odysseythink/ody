#!/usr/bin/env python3
"""Add `capabilities: ProviderCapabilities::default(),` to every
`ModelProviderInfo { ... }` struct literal in Rust source files.
Skips destructuring patterns and existing capabilities fields."""

import re
import sys
from pathlib import Path


def is_struct_literal(lines: list[str], idx: int) -> bool:
    """Heuristic: `ModelProviderInfo {` is a struct literal if the line before
    it is an assignment `=`, a `let`, a return position, or a function argument.
    It is a pattern if preceded by `let ModelProviderInfo {` or `if let ...`.
    """
    before = "".join(lines[max(0, idx - 3) : idx])
    text = before + lines[idx]
    # Pattern matching forms
    if re.search(r"\blet\s+ModelProviderInfo\s*\{", text):
        return False
    if re.search(r"\bfn\s+\w+\s*\([^)]*\)\s*(->\s*\S+)?\s*\{?\s*$", before, re.DOTALL):
        # Could be function body start followed by literal on next line
        pass
    return True


def add_capabilities_to_file(path: Path) -> int:
    raw = path.read_text(encoding="utf-8")
    lines = raw.splitlines(keepends=True)
    changed = 0
    i = 0
    while i < len(lines):
        if re.search(r"\bModelProviderInfo\s*\{", lines[i]) and is_struct_literal(lines, i):
            # Find matching close brace (handling nested braces)
            start = i
            brace_count = 0
            j = i
            found_open = False
            while j < len(lines):
                for k, ch in enumerate(lines[j]):
                    if ch == "{":
                        brace_count += 1
                        found_open = True
                    elif ch == "}":
                        brace_count -= 1
                        if found_open and brace_count == 0:
                            # This is the closing brace of the struct literal
                            break
                if found_open and brace_count == 0:
                    break
                j += 1
            else:
                i += 1
                continue

            block_lines = lines[i : j + 1]
            block_text = "".join(block_lines)
            # Skip if already has capabilities
            if "capabilities:" in block_text:
                i = j + 1
                continue

            # Insert capabilities before the closing `}`
            last_line = lines[j]
            # Find the position of the final `}`
            closing_idx = last_line.rfind("}")
            if closing_idx == -1:
                i = j + 1
                continue
            # Determine indentation of the closing brace
            indent_match = re.match(r"^(\s*)", last_line)
            indent = indent_match.group(1) if indent_match else ""
            inner_indent = indent + "    "
            new_last_line = (
                last_line[:closing_idx]
                + f"{inner_indent}capabilities: ProviderCapabilities::default(),\n{indent}}}"
                + last_line[closing_idx + 1 :]
            )
            lines[j] = new_last_line
            changed += 1
            i = j + 1
        else:
            i += 1

    if changed:
        path.write_text("".join(lines), encoding="utf-8")
    return changed


def main() -> int:
    root = Path(".")
    count = 0
    for path in root.rglob("*.rs"):
        n = add_capabilities_to_file(path)
        if n:
            print(f"{path}: updated {n} literal(s)")
            count += n
    print(f"Total literals updated: {count}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

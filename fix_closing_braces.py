#!/usr/bin/env python3
"""Fix indentation of struct closing braces after inserted capabilities."""

import re
import sys
from pathlib import Path


def fix_file(path: Path) -> bool:
    raw = path.read_text(encoding="utf-8")
    had_crlf = "\r\n" in raw
    text = raw.replace("\r\n", "\n")

    lines = text.splitlines(keepends=True)
    changed = False
    i = 0
    while i < len(lines):
        stripped = lines[i].rstrip("\n").lstrip()
        if stripped == "capabilities: ModelCapabilities::default(),":
            # Find the struct opening indentation by looking backward for "ModelInfo {"
            struct_indent = None
            brace_count = 0
            for j in range(i, -1, -1):
                line = lines[j].rstrip("\n")
                # Count braces from right to left
                for ch in reversed(line):
                    if ch == "}":
                        brace_count += 1
                    elif ch == "{":
                        brace_count -= 1
                        if brace_count < 0 and "ModelInfo {" in line:
                            struct_indent = re.match(r"^(\s*)", line).group(1)
                            break
                if struct_indent is not None:
                    break
            if struct_indent is not None:
                # The next non-empty line should be the closing brace; fix its indentation.
                for k in range(i + 1, len(lines)):
                    if lines[k].strip():
                        if lines[k].strip() == "}":
                            lines[k] = struct_indent + "}\n"
                            changed = True
                        break
        i += 1

    if changed:
        new_text = "".join(lines)
        out = new_text.replace("\n", "\r\n") if had_crlf else new_text
        path.write_text(out, encoding="utf-8")
        return True
    return False


def main() -> int:
    files = [
        "./app-server/tests/common/models_cache.rs",
        "./core/src/client_tests.rs",
        "./core/tests/suite/auto_review.rs",
        "./core/tests/suite/model_switching.rs",
        "./core/tests/suite/models_cache_ttl.rs",
        "./core/tests/suite/personality.rs",
        "./core/tests/suite/remote_models.rs",
        "./core/tests/suite/rmcp_client.rs",
        "./core/tests/suite/spawn_agent_description.rs",
        "./core/tests/suite/view_image.rs",
        "./tools/src/tool_config_tests.rs",
        "./models-manager/src/model_info.rs",
        "./ody-api/tests/models_integration.rs",
    ]
    for p in files:
        path = Path(p)
        if fix_file(path):
            print(f"updated {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

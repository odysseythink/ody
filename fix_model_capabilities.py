#!/usr/bin/env python3
"""Add ModelCapabilities import to files that use it, and fix indentation
of inserted `capabilities: ModelCapabilities::default(),` lines."""

import re
import sys
from pathlib import Path


def add_import(text: str) -> str | None:
    # Find existing ody_protocol::odysseythink_models imports.
    lines = text.splitlines(keepends=True)
    insert_idx = None
    last_ody_idx = None
    for i, line in enumerate(lines):
        if "use ody_protocol::odysseythink_models::" in line:
            last_ody_idx = i
        if "use ody_protocol::odysseythink_models::ModelCapabilities;" in line:
            return None
    if last_ody_idx is None:
        return None
    # Insert after the last ody_protocol::odysseythink_models import.
    lines.insert(last_ody_idx + 1, "use ody_protocol::odysseythink_models::ModelCapabilities;\n")
    return "".join(lines)


def fix_indentation(text: str) -> str:
    lines = text.splitlines(keepends=True)
    new_lines = []
    for i, line in enumerate(lines):
        stripped = line.rstrip("\n").lstrip()
        if stripped == "capabilities: ModelCapabilities::default(),":
            # Find indentation of previous non-empty non-comment line.
            prev_indent = ""
            for j in range(i - 1, -1, -1):
                prev = lines[j].rstrip()
                if prev and not prev.strip().startswith("//"):
                    prev_indent = re.match(r"^(\s*)", lines[j]).group(1)
                    break
            new_lines.append(prev_indent + stripped + "\n")
        else:
            new_lines.append(line)
    return "".join(new_lines)


def process_file(path: Path) -> bool:
    raw = path.read_text(encoding="utf-8")
    had_crlf = "\r\n" in raw
    text = raw.replace("\r\n", "\n")

    if "capabilities: ModelCapabilities::default()," not in text:
        return False

    new_text = fix_indentation(text)
    new_text = add_import(new_text) or new_text

    if new_text != text:
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
    ]
    for p in files:
        path = Path(p)
        if process_file(path):
            print(f"updated {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

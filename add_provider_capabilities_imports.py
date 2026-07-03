#!/usr/bin/env python3
"""Add `use ody_model_provider_info::ProviderCapabilities;` to files that
reference `ProviderCapabilities::default()` without importing it."""

import re
import sys
from pathlib import Path


def add_import(path: Path) -> bool:
    raw = path.read_text(encoding="utf-8")
    had_crlf = "\r\n" in raw
    text = raw.replace("\r\n", "\n")

    if "ProviderCapabilities::default()" not in text:
        return False
    if re.search(r"use\s+.*ProviderCapabilities", text):
        return False
    if "pub struct ProviderCapabilities" in text:
        return False

    lines = text.splitlines(keepends=True)
    insert_idx = None
    last_ody_idx = None
    for i, line in enumerate(lines):
        if re.search(r"use\s+ody_model_provider_info::", line):
            last_ody_idx = i
    if last_ody_idx is None:
        return False

    lines.insert(last_ody_idx + 1, "use ody_model_provider_info::ProviderCapabilities;\n")
    new_text = "".join(lines)
    out = new_text.replace("\n", "\r\n") if had_crlf else new_text
    path.write_text(out, encoding="utf-8")
    return True


def main() -> int:
    root = Path(".")
    for path in root.rglob("*.rs"):
        # Skip the crate that defines ProviderCapabilities.
        if "model-provider-info" in path.parts:
            continue
        if add_import(path):
            print(f"updated {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

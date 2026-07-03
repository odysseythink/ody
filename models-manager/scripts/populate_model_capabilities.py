#!/usr/bin/env python3
"""Populate ModelCapabilities.capabilities for every model in models.json.

The script derives capability values from existing top-level fields so the
nested object stays in sync with the legacy flat fields. It is safe to re-run.
"""
import json
import sys
from pathlib import Path

MODELS_JSON = Path(__file__).resolve().parent.parent / "models.json"


def derive_capabilities(model: dict) -> dict:
    modalities = model.get("input_modalities", ["text"])
    reasoning_levels = model.get("supported_reasoning_levels", [])
    thinking_effort = [level["effort"] for level in reasoning_levels if "effort" in level]
    supports_thinking = bool(reasoning_levels)
    supports_vision = "image" in modalities
    supports_tools = bool(model.get("supports_parallel_tool_calls", False))

    caps = {
        "context_window": model.get("context_window"),
        "max_context_window": model.get("max_context_window"),
        "max_output_tokens": None,
        "effective_context_window_percent": 95,
        "input_modalities": modalities,
        "supports_thinking": supports_thinking,
        "thinking_effort": thinking_effort,
        "supports_tools": supports_tools,
        "supports_parallel_tool_calls": model.get("supports_parallel_tool_calls", False),
        "supports_vision": supports_vision,
        "supports_image_detail_original": model.get("supports_image_detail_original", False),
        "supports_search_tool": model.get("supports_search_tool", False),
        "web_search_tool_type": model.get("web_search_tool_type", "text"),
        "supports_reasoning_summaries": model.get("supports_reasoning_summaries", False),
        "shell_type": model.get("shell_type", "default"),
        "tool_mode": None,
        "supports_multiple_system_messages": False,
        "supports_turn_pause": False,
        "truncation_policy": model.get("truncation_policy", {"mode": "bytes", "limit": 0}),
        "auto_compact_token_limit": model.get("auto_compact_token_limit"),
    }

    # Omit null / empty / default values to keep the JSON compact.
    def clean(value):
        if value is None:
            return None
        if isinstance(value, list) and not value:
            return None
        if isinstance(value, dict):
            cleaned = {k: clean(v) for k, v in value.items()}
            return cleaned if cleaned else None
        return value

    return {k: v for k, v in caps.items() if clean(v) is not None}


def main() -> int:
    raw = MODELS_JSON.read_text(encoding="utf-8")
    data = json.loads(raw)
    models = data.get("models", [])
    if not models:
        print("No models found in models.json", file=sys.stderr)
        return 1

    before = len(raw.encode("utf-8"))
    changed = 0
    for model in models:
        new_caps = derive_capabilities(model)
        if model.get("capabilities") != new_caps:
            model["capabilities"] = new_caps
            changed += 1

    after_text = json.dumps(data, ensure_ascii=False, indent=2) + "\n"
    after = len(after_text.encode("utf-8"))
    MODELS_JSON.write_text(after_text, encoding="utf-8")
    print(
        f"Updated {changed}/{len(models)} models. "
        f"Size: {before} -> {after} bytes ({100 * (after - before) / before:.1f}%)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

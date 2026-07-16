#!/usr/bin/env bash
# Clean only this workspace's own crates, keeping third-party dependency
# artifacts (e.g. the compiled `v8` crate) intact in target/.
#
# Use this instead of `cargo clean` to avoid recompiling v8 after every clean.
set -euo pipefail

cd "$(dirname "$0")/.."

packages=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
  | grep -o '"name":"[^"]*","version"' \
  | sed 's/"name":"//; s/","version"//' \
  | sort -u)

if [ -z "$packages" ]; then
  echo "error: no workspace packages found (is cargo metadata working?)" >&2
  exit 1
fi

args=()
while IFS= read -r pkg; do
  args+=(-p "$pkg")
done <<< "$packages"

echo "Cleaning $(printf '%s\n' "$packages" | wc -l | tr -d ' ') workspace packages (dependencies like v8 are kept)..."
cargo clean "${args[@]}"

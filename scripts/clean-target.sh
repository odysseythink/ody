#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

TARGET_DIR="${ROOT_DIR}/target"

if [[ ! -d "${TARGET_DIR}" ]]; then
  echo "No target directory found, nothing to clean."
  exit 0
fi

INCREMENTAL_DIR="${TARGET_DIR}/debug/incremental"
if [[ -d "${INCREMENTAL_DIR}" ]]; then
  echo "Removing incremental directory: ${INCREMENTAL_DIR}"
  rm -rf "${INCREMENTAL_DIR}"
fi

DEPS_DIR="${TARGET_DIR}/debug/deps"
if [[ -d "${DEPS_DIR}" ]]; then
  echo "Deleting files in ${DEPS_DIR} older than 2 days"
  find "${DEPS_DIR}" -type f -mtime +2 -delete
  # Also remove empty directories left behind
  find "${DEPS_DIR}" -mindepth 1 -type d -empty -delete 2>/dev/null || true
fi

echo "Target cleanup done."

#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${REPO_ROOT}/shared/protocol.js"
TARGETS=(
  "${REPO_ROOT}/mods/hotpot/protocol.js"
  "${REPO_ROOT}/mods/hotpot-session/protocol.js"
)

if [[ "${1:-}" == "--check" ]]; then
  for target in "${TARGETS[@]}"; do
    if [[ ! -f "${target}" ]]; then
      echo "ERROR: Missing synced file: ${target} (run ./scripts/sync.sh)" >&2
      exit 1
    fi
    if ! cmp -s "${SRC}" "${target}"; then
      echo "ERROR: Out-of-sync file: ${target} differs from ${SRC} (run ./scripts/sync.sh)" >&2
      exit 1
    fi
  done
  echo "OK: shared/protocol.js is in sync across all mods."
  exit 0
fi

for target in "${TARGETS[@]}"; do
  mkdir -p "$(dirname "${target}")"
  cp "${SRC}" "${target}"
  echo "Synced ${SRC} -> ${target}"
done

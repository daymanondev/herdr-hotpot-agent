#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# A shared module (source of truth in `shared/`) and the mod-local copies it
# must stay byte-identical with: a Herdr mod cannot import outside its own
# `mods/<name>/` directory, so shared helpers are synced into every mod.
SYNC_PAIRS=(
  "shared/protocol.js:mods/hotpot/protocol.js mods/hotpot-session/protocol.js"
  "shared/chrome.js:mods/hotpot/chrome.js mods/hotpot-session/chrome.js"
)

if [[ "${1:-}" == "--check" ]]; then
  for pair in "${SYNC_PAIRS[@]}"; do
    src="${REPO_ROOT}/${pair%%:*}"
    for target in ${pair#*:}; do
      target_path="${REPO_ROOT}/${target}"
      if [[ ! -f "${target_path}" ]]; then
        echo "ERROR: Missing synced file: ${target_path} (run ./scripts/sync.sh)" >&2
        exit 1
      fi
      if ! cmp -s "${src}" "${target_path}"; then
        echo "ERROR: Out-of-sync file: ${target_path} differs from ${src} (run ./scripts/sync.sh)" >&2
        exit 1
      fi
    done
  done
  echo "OK: shared modules are in sync across all mods."
  exit 0
fi

for pair in "${SYNC_PAIRS[@]}"; do
  src="${REPO_ROOT}/${pair%%:*}"
  for target in ${pair#*:}; do
    target_path="${REPO_ROOT}/${target}"
    mkdir -p "$(dirname "${target_path}")"
    cp "${src}" "${target_path}"
    echo "Synced ${src} -> ${target_path}"
  done
done

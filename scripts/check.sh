#!/usr/bin/env bash
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${HOME}/.local/bin:/opt/homebrew/bin:${PATH}"
if [[ -d "${HOME}/.nvm/versions/node" ]]; then
  LATEST_NODE="$(ls -1 "${HOME}/.nvm/versions/node" | tail -n 1)"
  if [[ -n "${LATEST_NODE}" ]]; then
    export PATH="${HOME}/.nvm/versions/node/${LATEST_NODE}/bin:${PATH}"
  fi
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

echo "==> [1/6] Checking shared protocol.js sync across Herdr mods..."
./scripts/sync.sh --check

echo "==> [2/6] Checking JavaScript syntax in shared/ and mods/..."
JS_FILES=(
  "shared/protocol.js"
  "mods/hotpot/protocol.js"
  "mods/hotpot/main.js"
  "mods/hotpot-session/protocol.js"
  "mods/hotpot-session/main.js"
)
for js in "${JS_FILES[@]}"; do
  node --input-type=module --check < "${js}"
  echo "  OK: ${js}"
done

echo "==> [3/6] Running node tests for the shared view-model helpers..."
node --test tests/js/*.test.mjs

echo "==> [4/6] Running node tests (mod view model, keymap, static contract)..."
node --test 'tests/mods/*.test.mjs'

echo "==> [5/6] Checking JSON manifests (layouts, mod.json, keys.json)..."
JSON_FILES=(
  "layouts/hotpot.json"
  "mods/hotpot/mod.json"
  "mods/hotpot-session/mod.json"
  "keys.json"
)
for jf in "${JSON_FILES[@]}"; do
  node -e "JSON.parse(require('fs').readFileSync(process.argv[1], 'utf8'));" "${jf}"
  echo "  OK: ${jf}"
done

echo "==> [6/6] Running Rust test suite (cargo test)..."
cargo test

echo "==> All checks passed!"

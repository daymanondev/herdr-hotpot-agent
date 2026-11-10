#!/usr/bin/env bash
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${HOME}/.local/bin:/opt/homebrew/bin:${PATH}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HERDR_ROOT="${HOME}/.config/herdr-app"
BIN_DIR="${HOME}/.local/bin"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --root)
      HERDR_ROOT="$2"
      BIN_DIR="$2/bin"
      shift 2
      ;;
    *)
      echo "Usage: $0 [--root <target-herdr-config-dir>]" >&2
      exit 1
      ;;
  esac
done

cd "${REPO_ROOT}"

echo "==> [1/4] Syncing shared/protocol.js into mods/..."
./scripts/sync.sh

echo "==> [2/4] Building Rust hotpot binary (release)..."
cargo build --release

mkdir -p "${BIN_DIR}"
cp "${REPO_ROOT}/target/release/hotpot" "${BIN_DIR}/hotpot"
chmod +x "${BIN_DIR}/hotpot"
echo "  Installed binary: ${BIN_DIR}/hotpot"

echo "==> [3/4] Installing Herdr layout and mods into ${HERDR_ROOT}..."
mkdir -p "${HERDR_ROOT}/layouts" "${HERDR_ROOT}/mods/hotpot" "${HERDR_ROOT}/mods/hotpot-session"
cp "${REPO_ROOT}/layouts/hotpot.json" "${HERDR_ROOT}/layouts/hotpot.json"
cp "${REPO_ROOT}/mods/hotpot/mod.json" "${HERDR_ROOT}/mods/hotpot/mod.json"
cp "${REPO_ROOT}/mods/hotpot/main.js" "${HERDR_ROOT}/mods/hotpot/main.js"
cp "${REPO_ROOT}/mods/hotpot/protocol.js" "${HERDR_ROOT}/mods/hotpot/protocol.js"
cp "${REPO_ROOT}/mods/hotpot-session/mod.json" "${HERDR_ROOT}/mods/hotpot-session/mod.json"
cp "${REPO_ROOT}/mods/hotpot-session/main.js" "${HERDR_ROOT}/mods/hotpot-session/main.js"
cp "${REPO_ROOT}/mods/hotpot-session/protocol.js" "${HERDR_ROOT}/mods/hotpot-session/protocol.js"

echo "==> [4/4] Verifying hotpot binary..."
"${BIN_DIR}/hotpot" status >/dev/null
echo "  Install complete! Activate in Herdr via settings.json: { \"layout\": \"hotpot\" } or see RECIPE.md."

# herdr-hotpot-agent

A multi-provider SLP verification factory and Paseo-style session inspector built for **Herdr App (`herdr-app` + `herdr-core` / `hc`)**, pairing a native Rust engine (`hotpot`) with GPU-rendered `gpui-kit` window mods.

## Architecture & Runtime Rules

- **Two-part runtime**:
  1. **`hotpot` (Rust binary in `src/`)**: Single-binary engine handling the SLP `Ledger` (`ledger.jsonl`), `Mailbox` (`mailbox.jsonl`), `Task worktree` lifecycle, detached `Gate` execution, role-scoped stdio MCP server (`hotpot mcp`), `< 1.5ms` `PreToolUse` choke hook (`hotpot hook`), Layer-3 `git` shim (`hotpot git-shim`), clean-room provider home provisioning (`claude`, `codex`, `opencode`, `pi`, `agy`, `copilot`, `zcode`), and real-time structured session/thinking extraction (`hotpot inspect`).
  2. **Herdr GPUI Mods (`mods/hotpot/` and `mods/hotpot-session/`)**: Written in Herdr's `gpui-kit` JavaScript (`View`, `div`, `v_flex`, `h_flex`, `TextView.markdown`, `Editor`). Mods describe GPU elements drawn natively by `herdr-app` — never use HTML, CSS, DOM APIs, Node built-ins, or `setTimeout`/`setInterval` inside `mods/` (use `cx.timer`, `cx.spawn`, `exec`, `hc`, `readTextFile`, and `watchFolder` from `"herdr"`).
- **Shared mod modules**: A `.js` helper shared across multiple mods (`shared/*.js`) has a single source of truth in `shared/` because a Herdr mod cannot import outside its own `mods/<name>/` directory. Edit in `shared/`, then run `./scripts/sync.sh`. `./scripts/check.sh` fails if any synced copy diverges.
- **Safe installation & testing**: Never overwrite a live `~/.config/herdr-app` during automated tests or unverified experiments. Use `./scripts/install.sh --root <scratch-dir>` for dry runs, and run `./scripts/check.sh` before every commit.
- **Documentation language**: Every document, ADR, code comment, and issue in this repository is written in **English**.

## Verification Loop

Run the full check suite (takes seconds):

```bash
./scripts/check.sh
```

This verifies:
1. `./scripts/sync.sh --check` (shared mod files are in sync).
2. `cargo test` & `cargo clippy --all-targets` (Rust engine, Choke policy, Clean-Room generator, and Session Inspector).
3. `node --input-type=module --check` across every `.js` file in `shared/` and `mods/`.

When a live `herdr-app` window is running on macOS, also run:

```bash
herdr-app check ~/.config/herdr-app/mods/hotpot
herdr-app check ~/.config/herdr-app/mods/hotpot-session
```

## Docs

- **Domain vocabulary**: `GLOSSARY.md` at the repo root.
- **Architecture decisions**: `docs/adr/`.
- **Deep-dive design**: `docs/architecture/herdr-slp-synthesis.md`.
- **Herdr Gallery recipe**: `RECIPE.md`.

## Agent skills

### Issue tracker

Issues and specs live as local markdown files under `.scratch/<feature-slug>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as `Status:` lines in issue files. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

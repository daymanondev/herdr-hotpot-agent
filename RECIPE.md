# Hotpot

A multi-provider SLP verification factory and Paseo-style session inspector docked on the right side of Herdr. Spaces and agents stay on the left, your active terminals or the structured Session Inspector sit in the center, and **Hotpot** on the right orchestrates `Supervisor`, `Lead`, and isolated worktree `Peers` across Claude Code, Codex, OpenCode, Pi, Gemini/Antigravity, Copilot, and ZCode.

![Hotpot](preview.png)

## What you see

Left to right, building on Herdr's default `main` layout:

- **Sidebar** (268px): Herdr's brand bar (`workspace-chrome`), `spaces`, `agents`, and footer.
- **Center Work Column**:
  - Top tab bar (`tabs`, 42px).
  - Center stage (`main-stack`): toggles seamlessly between Herdr's active terminal tabs (`{ "tab": "active" }`) and the **Hotpot Session Inspector** (`hotpot-session`), which renders collapsible **💭 Thinking** blocks, **🔧 Tool Calls** (`input` / `output`), **📩 Mailbox** envelopes, and a live prompt box for the selected `Supervisor`, `Lead`, or `Peer`.
- **Right Dock — Hotpot Panel** (`hotpot`, 360px default, resizable 280–600px, radius 10, `chrome` background):
  - **Seats**: Live cards for `Supervisor` and `Lead` showing provider/model lane, status dot (`working`, `blocked`, `done`, `idle`), and context-window occupancy (`64k / 128k`).
  - **Tasks & Peers**: Active SLP waves and Tasks (`T-01`, `T-02`, ...), their disposition (`engineer`, `scout`, `architect`, `reviewer`), Attempt counter (`1/3`), Gate Verdict pill (`GREEN` / `RED` / `RUNNING`), and live activity snippet (`💭 Thinking...` or `🔧 cargo test`).
  - **Live Mailbox (`Lead ↔ Peer`)**: Real-time stream of `directive`, `claim`, `blocker`, and `verdict` messages.
  - **Status Bar** (`workspace-status`, 28px) along the bottom.

## How it behaves

- **Click `Supervisor`, `Lead`, or any `Peer` row** in the right-hand Hotpot panel:
  - Opens that agent's **Session Inspector** (`hotpot-session`) in the center stage, streaming full `Thinking` blocks, tool invocations, and token metrics directly from the provider's on-disk session store via `hotpot inspect`.
  - Clicking **Terminal (`Raw PTY`)** in the inspector header (or pressing `Alt+T`) switches the center stage to the live `hc` terminal for that seat or Peer (`pane.focus` / `terminal.peek`).
- **Role-scoped MCP & Skill Isolation**:
  - When `Lead` calls `lead_spawn_agent`, the `hotpot` Rust binary allocates an isolated Git worktree (`<stateDir>/projects/<slug>/worktrees/<task>`), materializes a clean-room provider home with strictly role-scoped MCP tools (`hotpot mcp --role agent`) and contract-scoped `SKILL.md` links, and launches the agent in `herdr-core` (`hc run --bg --keep`).
- **Sub-millisecond 3-Layer Choke**:
  - Every tool call passes through `hotpot hook` (`< 1.5ms` native Rust binary) enforcing the **Seat Write Fence** on `Supervisor`/`Lead` and worktree + `--scope` boundaries on `Peer`, backed by `hotpot git-shim` on `PATH`.

## Built from

- `layouts/hotpot.json`: Cloned from `main` (`0.1.12`) with `main-stack` (switching between active tab terminals and `hotpot-session`) and the right-docked `hotpot` block (`360px`).
- `mods/hotpot/`: The right-hand SLP control tower and topology panel (`place: "right"`, `size: 360`).
- `mods/hotpot-session/`: The Paseo-style structured session and thinking viewer (`place: "none"`).
- `src/` (`Cargo.toml` → `hotpot` binary): Native Rust engine driving `hc`, SLP ledger/spool, clean-room provider homes, `PreToolUse` choke hook, stdio MCP bridge, and O(delta) JSONL/SQLite transcript reader.
- Shipped Herdr mods used: `workspace-chrome`, `spaces`, `agents`, `tabs`, `pane`, `files`, `changes`, `workspace-status`.

## Watch out

- Install with `./scripts/install.sh` (or `./scripts/install.sh --root <scratch>` for testing) so the `hotpot` Rust binary is compiled and linked into `~/.local/bin/hotpot` and shared JS modules are synced into `mods/`.
- `mods/hotpot` and `mods/hotpot-session` invoke `exec("hotpot", ...)` and `hc(...)` and watch `<stateDir>/projects/` via `watchFolder`.

## Done when

- `./scripts/check.sh` prints `all checks passed`.
- `herdr-app check ~/.config/herdr-app/mods/hotpot` and `herdr-app check ~/.config/herdr-app/mods/hotpot-session` both exit 0.
- `herdr-app do layout.switch '{"name":"hotpot"}'` shows the Hotpot panel docked on the right with zero layout errors, and clicking a seat opens its structured Session Inspector with live `Thinking` blocks.

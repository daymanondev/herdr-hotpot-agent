# ADR 0001: Hybrid Native Rust Engine (`hotpot`) and Herdr GPUI Mods (`mods/*`)

- **Status**: accepted

We split **herdr-hotpot-agent** into a single native Rust binary (`hotpot` in `src/`) and two GPU-rendered Herdr window mods (`mods/hotpot/` and `mods/hotpot-session/`).

## Context

Herdr consists of two cooperating native programs:
1. **`herdr-app`**: A native GPU desktop window built on GPUI (`gpui-kit`), configured through JSON layouts (`layouts/<name>.json`) and JavaScript `View` mods (`mods/<name>/main.js`) that describe native GPU elements (`div()`, `v_flex()`, `h_flex()`, `TextView.markdown()`, `Editor.new()`).
2. **`herdr-core` (`hc`)**: A persistent background runtime that owns every PTY terminal and agent process across window restarts (`hc run`, `hc ls`, `hc get`, `hc send`, `hc wait`, `hc kill`, `hc rm`, `hc label`).

We need to bring `paseo-room`'s orchestration model (`Human / Supervisor / Lead / Peer`, isolated Role Homes under `~/.hotpot`, commit-bound Assignments, Attention Letters, and structured Session/Thinking inspection) into Herdr with native speed and zero external daemon overhead.

## Decision

1. **`hotpot` (Single Native Rust Binary in `src/`)**:
   - Compiled via `Cargo.toml` and installed to `~/.local/bin/hotpot` (and `mods/hotpot/bin/hotpot`).
   - Owns all provisioning, runtime coordination, MCP serving, and session inspection under **`~/.hotpot/`** (overridable via `HOTPOT_HOME`):
     - `hotpot setup [--agent codex|claude|pi] [--runtime] [--apply]`, `hotpot verify`, `hotpot auth login <agent> <role>`, and `hotpot remove [--apply]`: Provisions isolated `Role Homes` under `~/.hotpot/roles/<agent>/<role>/` with capability hygiene and additive role contracts.
     - `hotpot mcp --role <supervisor|lead|peer>`: Zero-dependency stdio JSON-RPC 2.0 MCP server exposing role-asymmetric coordination tools (`room_status`, `message_lead`, `attention_feedback`, `lead_replace_*` for `Supervisor`; `assignment_*`, `gate_run`, `workspace_close`, `lease_reclaim` for `Lead`; strictly `ask` and `handoff` for `Peer`).
     - `hotpot status --workspace <dir>`: Emits a JSON snapshot of the `Project`'s `Supervisor`, `Lead`, active `Peers`, `Assignments`, `Attention Letters`, and context token usage.
     - `hotpot inspect --agent <id>`: Reads the agent's on-disk `.jsonl` session store under `~/.hotpot/` to emit structured `Thinking`, `Tool Call`, and `Message` blocks plus token breakdown (`Cache Read` / `Input` / `Output`).

2. **Herdr GPUI Window Mods (`mods/hotpot/` and `mods/hotpot-session/`) + Layout (`layouts/hotpot.json`)**:
   - `mods/hotpot/`: Docked on the right side of the Herdr window (`place: "right"`, `size: 360`). Watches `~/.hotpot/runtime/v1/` via Herdr's native `watchFolder` and calls `exec("hotpot", ["status", ...])`.
   - `mods/hotpot-session/`: Placed in the center `main-stack` of `layouts/hotpot.json`, toggling between structured `Thinking` / `Tool Call` inspection and the live `hc` PTY terminal (`pane.focus` / `terminal.peek`).

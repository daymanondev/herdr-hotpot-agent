# herdr-hotpot-agent

A multi-provider verification factory and Paseo-style session inspector for **Herdr App (`herdr-app` + `herdr-core` / `hc`)**, powered by a single native Rust engine (`hotpot`) and GPU-rendered `gpui-kit` mods.

Why **Hotpot**? Because it simmers the best ingredients from across the agentic coding ecosystem into one pot:
1. **Herdr's persistent terminal & GPU window runtime** (`hc` + `herdr-app` `gpui-kit` mods).
2. **Multi-provider coding agents** (`claude`, `codex`, `opencode`, `pi`, `agy` / Gemini, `copilot`, `zcode`) running side by side across role lanes.
3. **SLP (Self-driving Loop Protocol)** verification discipline (`Owner → Supervisor → Lead → Peer`, 4-Part Contracts, isolated Git worktrees, mechanical Gates, 6-Field Handoffs, and in-place blocker reconciliation).
4. **Paseo-style Session & Thinking Inspector** — click `Supervisor`, `Lead`, or any `Peer` in the right-hand Hotpot panel to inspect its structured conversation stream (`💭 Thinking`, `🔧 Tool Calls`, `📩 Mailbox`, and live prompt input) or toggle immediately to its raw interactive `hc` terminal.
5. **Sub-millisecond Rust Security & Context Engine (`hotpot`)** — 3-Layer Choke (`PreToolUse` hooks + `PATH` `git` shim), Clean-Room MCP & Skill isolation per role/task, and O(delta) context-window sensors (`128k` Lead / `160k` Peer).

---

## Repository Layout

```text
herdr-hotpot-agent/
├── Cargo.toml                  # Single Rust binary (`hotpot`): engine, MCP server, PreToolUse hook, session inspector
├── src/                        # Rust source code
│   ├── main.rs                 # CLI entrypoint (`status`, `inspect`, `spawn-seat`, `spawn-peer`, `mcp`, `hook`, `git-shim`)
│   ├── hc.rs                   # Bridge to `herdr-core` (`hc run`, `hc ls`, `hc send`, `hc wait`, `hc kill`)
│   ├── slp/                    # SLP state machine: Ledger, 4-Part Contracts, Mechanical Gate runner, Mailbox, Worktrees
│   ├── guard/                  # 3-Layer Choke, Clean-Room Provider Homes (MCP & Skill isolation), PreToolUse hook
│   └── inspector/              # Real-time JSONL/SQLite reader for Thinking blocks, Tool Calls, and Token Usage
├── layouts/
│   └── hotpot.json             # Herdr App layout: Spaces/Agents on left, Active Tab / Session Inspector center, Hotpot panel right
├── mods/
│   ├── hotpot/                 # Right-docked GPUI panel: Seats, Tasks, Peers, Live Mailbox, Context & Gate badges
│   │   ├── mod.json            # `{ "place": "right", "size": 360 }`
│   │   └── main.js             # GPUI View (`gpui-kit`)
│   └── hotpot-session/         # Paseo-style Session Inspector mod: collapsible Thinking, Tool Calls, Diffs, and Chat input
│       ├── mod.json            # `{ "place": "none" }`
│       └── main.js             # GPUI View (`gpui-kit`)
├── shared/                     # Shared JS helpers synced into `mods/*/` via `scripts/sync.sh`
├── scripts/
│   ├── sync.sh                 # Syncs `shared/*.js` into mod folders
│   ├── check.sh                # Runs `cargo check`, `cargo test`, `sync.sh --check`, and `node --check` on all mods
│   └── install.sh              # Builds `hotpot` release binary and installs layout/mods into `~/.config/herdr-app` (or `--root <dir>`)
├── keys.json                   # Optional Herdr shortcuts for toggling the Hotpot panel and Session Inspector
├── RECIPE.md                   # Herdr Gallery specification (`herdr.dev/gallery`)
├── AGENTS.md                   # Agent engineering rules and Matt Pocock skills pointers
├── GLOSSARY.md                 # Canonical domain vocabulary
└── docs/
    ├── adr/                    # Architecture Decision Records
    ├── agents/                 # Matt Pocock skill configuration (`issue-tracker.md`, `triage-labels.md`, `domain.md`)
    └── architecture/           # Deep-dive technical specifications
```

---

## Quick Start

### 1. Build & Run Verification Checks

```bash
./scripts/check.sh
```

### 2. Try in a Scratch Herdr Home (Safe Sandbox)

```bash
./scripts/install.sh --root /tmp/herdr-hotpot-try
```

### 3. Install into Live Herdr App (`~/.config/herdr-app`)

```bash
./scripts/install.sh
herdr-app check ~/.config/herdr-app/mods/hotpot
herdr-app check ~/.config/herdr-app/mods/hotpot-session
herdr-app do layout.switch '{"name":"hotpot"}'
```

# Architecture Synthesis: Bringing Paseo & SLP into Herdr (`herdr-hotpot-agent`)

## 1. Executive Summary

**herdr-hotpot-agent** unifies **Herdr** (`herdr-app` + `herdr-core` / `hc`) with the **Self-driving Loop Protocol (SLP)** verification architecture into a single native Rust engine (`hotpot`) and two GPU-rendered Herdr mods (`mods/hotpot` and `mods/hotpot-session`).

```text
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                        HERDR APP WINDOW (Rust GPUI + gpui-kit)                       │
│ ┌─────────────────┬────────────────────────────────────────┬───────────────────────┐ │
│ │ Left Sidebar    │ Center Stage (`main-stack`)            │ Right Dock (`hotpot`) │ │
│ │ • Spaces        │ [Mode A: Paseo-Style Session Inspector]│ • Seats:              │ │
│ │ • Agents        │   📩 Mailbox Directive                 │   🟢 Supervisor       │ │
│ │                 │   💭 Thinking (full collapsible block) │   🟣 Lead [64k/128k]  │ │
│ │                 │   🔧 Tool Call: lead_set_contract      │ • Tasks & Peers:      │ │
│ │                 │   💬 Live Prompt Input -> `hc send`    │   ✅ T-01 (GREEN)     │ │
│ │                 │ [Mode B: Active Tab Raw PTY Terminals] │   🔄 T-02 [Att 1/3]   │ │
│ │                 │   Interactive `hc` PTYs                │ • Live Mailbox Stream │ │
│ └─────────────────┴───────────────────┬────────────────────┴───────────┬───────────┘ │
└───────────────────────────────────────┼────────────────────────────────┼─────────────┘
                                        │ `exec("hotpot", ...)`          │ `watchFolder`
                                        ▼                                ▼
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                         HOTPOT RUST ENGINE (`hotpot` binary)                         │
│  • `hotpot status` / `hotpot inspect` (O(delta) JSONL & SQLite Thinking/Usage reader)│
│  • `hotpot spawn-seat` / `hotpot spawn-peer` (Clean-Room Homes + `hc run --bg`)      │
│  • `hotpot mcp` (Role-scoped stdio JSON-RPC 2.0 MCP server)                          │
│  • `hotpot hook` (< 1.5ms PreToolUse Choke, Seat Write Fence, & Skill/MCP Fence)     │
│  • `hotpot git-shim` (Layer-3 PATH git wrapper)                                      │
└───────────────────────────────────────┬──────────────────────────────────────────────┘
                                        │ `hc run` / `hc send` / `hc wait` / `.jsonl`
                                        ▼
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                    HERDR-CORE (`hc`) PERSISTENT TERMINAL RUNTIME                     │
│  • Terminal t1 (`herdr.seat=supervisor`)  • Terminal t2 (`herdr.seat=lead`)          │
│  • Terminal t3 (`herdr.seat=agent`, `herdr.task=T-02`, cwd=`.../worktrees/T-02`)     │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. How Multi-Provider Clean-Room Launching Works Over `hc`

When `hotpot spawn-seat` or `hotpot spawn-peer` launches an agent on a configured lane (`<provider>/<model>[:<thinking>]`), it first constructs a **Clean-Room Home** under `<stateDir>/homes/<provider>/<role>-<corr>/`:

| Provider Family | Clean-Room Mechanism | Role-Scoped MCP (`hotpot mcp`) | Skill Isolation (`SLP_ROLE_SKILLS` / `Contract.skills`) | Layer-2 Choke (`hotpot hook`) |
| :--- | :--- | :--- | :--- | :--- |
| **Claude Code (`claude`)** | `--settings <cleanRoom>/settings.json` | `--strict-mcp-config --mcp-config <cleanRoom>/mcp.json` | Symlinked `<cleanRoom>/skills/` + `PreToolUse` Skill/Read fence | `hooks.PreToolUse` in `settings.json` + `--disallowedTools` |
| **Codex (`codex`)** | `CODEX_HOME=<cleanRoom>` (symlinks `~/.codex/auth.json`) | `[mcp_servers.hotpot]` in `<cleanRoom>/config.toml` | Symlinked `<cleanRoom>/skills/` + `PreToolUse` Skill/Read fence | `hooks.json` (`PreToolUse`) + `--no-daemon` |
| **OpenCode (`opencode`)** | `OPENCODE_CONFIG_DIR=<cleanRoom>` | `"mcp": { "hotpot": ... }` in `opencode.json` | `"permission": { "skill": { "*": "deny", ... } }` in `opencode.json` | `plugins/hotpot-choke.js` (`tool.execute.before`) |
| **Pi (`pi`)** | `PI_CODING_AGENT_DIR=<cleanRoom>` | `<cleanRoom>/mcp.json` (`exposure: "direct"`) | `"skills": ["!*", "+..."]` in `<cleanRoom>/settings.json` | `<cleanRoom>/extensions/hotpot-choke.ts` (`tool_call`) |
| **Gemini / Antigravity (`agy`)** | Clean-room plugin config | `mcp_config.json` | `PreToolUse` Skill/Read fence | `hooks.json` (`PreToolUse`) |
| **Copilot (`copilot`)** | `COPILOT_HOME=<cleanRoom>` | `<cleanRoom>/mcp-config.json` | `preToolUse` Skill/Read fence | `preToolUse` in `<cleanRoom>/settings.json` |
| **ZCode (`zcode`)** | `ZCODE_STORAGE_DIR=<cleanRoom>` | `<cleanRoom>/config.json` | Symlinked `<cleanRoom>/skills/` + `<worktree>/.zcode/config.json` | `ZCODE_DISALLOWED_TOOLS` + guarded mode |

---

## 3. Blocking Unpermitted `MCP` and `SKILL` Access Between `Lead` and `Peer`

1. **No Direct `hc` or `herdr-app` Spawning by Agents**:
   - `Lead` cannot run `hc run` or `herdr-app do` via Bash. Both Layer 2 (`hotpot hook`) and Layer 3 (`<stateDir>/bin/hc` shim) reject direct orchestration calls with `seat-hc-tool-fence`.
   - `Lead` can only spawn a `Peer` by calling the `lead_spawn_agent` MCP tool.
2. **Strict MCP Scoping**:
   - `hotpot` generates a distinct `correlationId` (`cor_<uuid>`) for each terminal and passes `--role <supervisor|lead|agent> --correlation <corr>` to `hotpot mcp`.
   - A `Peer` terminal only sees `agent_claim` and `agent_escalate_blocker` in `tools/list` and is rejected if it attempts to call any `lead_*` or `supervisor_*` tool.
   - `Supervisor` and `Lead` receive only the `hotpot` MCP server (`--strict-mcp-config`), while `Peers` additionally receive enabled helper MCP servers (`chrome-devtools-mcp`, `expo-mcp`, `exa`, etc.).
3. **Two-Layer Skill Fencing**:
   - **Discovery Layer**: Each Clean-Room Home only links or allowlists the skills permitted for that role (`["codebase-design", "domain-modeling", "tdd"]` for `Lead`; Contract-granted domain skills minus `factory` for `Peer`).
   - **Execution Layer (`hotpot hook`)**: If `Lead` attempts to bypass discovery by calling `Read` or `Bash` (`cat`, `head`, `rg`) on `~/.agents/skills/<impl-skill>/SKILL.md` (or if `Peer` attempts to read `.../skills/factory/SKILL.md`), `hotpot hook` inspects the target path before execution and returns `permissionDecision: "deny"`.

---

## 4. Real-Time Thinking & Session Inspection (`hotpot inspect`)

When the Owner clicks `Lead` or `Peer` in the right-hand `hotpot` panel:
1. `mods/hotpot/main.js` updates `shared("hotpot.selected")` and switches `main-stack` in `layouts/hotpot.json` to `hotpot-session`.
2. `mods/hotpot-session/main.js` calls `exec("hotpot", ["inspect", "--project", slug, "--agent", agentId])` and watches the session directory via `watchFolder`.
3. `hotpot inspect` parses the agent's live on-disk session file (`.jsonl` or `.sqlite`), returning:
   - Full uncollapsed **`thinking` / `reasoning`** blocks.
   - Structured **`tool_call`** and **`tool_result`** payloads.
   - Interleaved **`mailbox.jsonl`** messages (`Supervisor → Lead`, `Lead → Peer`, `Peer → Lead`).
   - Exact **context token occupancy** (`latest_context_tokens` against the `128,000` Lead and `160,000` Peer rotation thresholds).
4. At any moment, clicking **Raw PTY** in the `hotpot-session` header switches `main-stack` back to `{ "tab": "active" }` and focuses the agent's live `hc` terminal (`run("pane.focus", { terminal })`).

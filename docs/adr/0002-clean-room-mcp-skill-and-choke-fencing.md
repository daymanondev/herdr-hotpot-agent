# ADR 0002: Role Home Provisioning (`~/.hotpot`) and Capability Hygiene over `hc`

- **Status**: accepted (supersedes earlier PreToolUse Choke draft)

Instead of intercepting every shell and file tool with a `PreToolUse` regex sandbox or restricting `Lead` to three design skills, `hotpot` implements `paseo-room`'s **Role Home Provisioning** and **Capability Hygiene** model under `~/.hotpot` on top of `herdr-core` (`hc`).

## Context

When `hc` runs coding agents (`codex`, `claude`, `pi`), `hc` + `hotpot` must remain the single control plane, prevent seats from sharing mutable credentials or spawning native subagents, and keep `Peer` focused on its assignment brief without orchestration clutter. An earlier prototype attempted to enforce this via a `PreToolUse` hook (`hotpot hook`) and a `PATH` `git-shim` that blocked shell commands and blocked `Lead` from reading implementation skills. In practice, regex shell hooks are brittle pseudo-sandboxes, and starving `Lead` of operator skills prevents effective framing and onboarding.

## Decision

1. **Single Control Plane & Role Homes (`~/.hotpot`)**:
   - `hotpot setup` (dry-run by default; mutates only with `--apply`) reads `~/.codex`, `~/.claude`, and `~/.pi/agent` without ever modifying them, and writes isolated role homes under `~/.hotpot/roles/<agent>/<supervisor|lead|peer>/` (overridable via `HOTPOT_HOME`).
   - Each role home owns its mutable credential store (`hotpot auth login <agent> <role>`) and session state. Config files are written atomically via temp-sibling rename.
   - Every native multi-agent surface is closed fail-closed:
     - **Codex**: `[agents].enabled = false`, `features.multi_agent = false`, `features.multi_agent_v2 = false`, and a generated `model-catalog.json` with `multi_agent_version` scrubbed.
     - **Claude**: Canonical native-orchestration deny list in both launch flags and `settings.json` (`permissions.deny`), plus `CLAUDE_CODE_DISABLE_AGENT_VIEW=1`, `CLAUDE_CODE_DISABLE_WORKFLOWS=1`, and `ENABLE_CLAUDEAI_MCP_SERVERS=0`.
     - **Pi**: `--no-extensions --extension builtin:mcp --extension builtin:codemode --no-approve`, verified by an offline RPC `/mcp` probe.

2. **Additive Three-Layer Instructions**:
   - Vendor base prompts are never replaced. Role contracts (`shared-authority`, `shared-seat-identity`, `challenge-signals`, and per-role body) are appended via `developer_instructions` (Codex), system prompt append + `CLAUDE.md` fallback (Claude), and `--append-system-prompt APPEND_SYSTEM.md` (Pi).
   - Repository-specific tactics live in an optional root `WORKSPACE_PROTOCOL.md` read only by `Lead` (never advertised to `Peer`).

3. **Skill Projection & Capability Hygiene**:
   - `Supervisor` links the operator's resources directly.
   - `Lead` links a managed skill aggregate under `~/.hotpot/skill-projections/<agent>/lead/` containing all non-colliding operator skills plus the built-in `hotpot-project-onboarding` skill.
   - `Peer` receives a filtered skill directory excluding orchestration skills (`paseo*`, `hotpot*`, `factory*`), withholds executable resources (`plugins`, `hooks`, `commands`, `prompts`), and receives only the assignment-scoped reporting MCP tools (`ask`, `handoff`).
   - One `Supervisor` is bound strictly 1:1 to one `Project` (one Herdr Space) to prevent cross-project context bleed.

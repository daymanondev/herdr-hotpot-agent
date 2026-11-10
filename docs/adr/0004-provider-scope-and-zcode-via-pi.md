# ADR 0004: Core Agent Adapters (`codex`, `claude`, `pi`) and ZCode via `zcode-api` + `pi`

- **Status**: accepted

`hotpot` ships first-class role-home adapters for **`codex`, `claude`, and `pi`**, and supports **ZCode** by routing `TriDefender/zcode-api` into `pi` (`models.json`) rather than maintaining a standalone ZCode ACP/CLI adapter.

## Context

Each supported coding agent requires fail-closed verification of its native multi-agent closures, credential isolation, additive prompt carriers, and MCP registration (`paseo-room` standard). Maintaining seven separate CLI adapters from day one dilutes verification quality. Meanwhile, ZCode/GLM coding plans are already exposed as local OpenAI/Anthropic/Responses endpoints on `127.0.0.1:8080` by `TriDefender/zcode-api`, and `pi` natively consumes custom local providers via `~/.pi/agent/models.json` (which `hotpot setup` links into every Pi role home).

## Consequences

- Slice 1 enforces complete fail-closed verification on `codex`, `claude`, and `pi`.
- ZCode coding plans run inside `pi` seats through `zcode-api`, inheriting Pi's verified MCP bridge, capability hygiene, and JSONL session inspector without a separate adapter surface.
- Additional native CLI adapters (`opencode`, `agy`, `copilot`) remain deferred until their multi-agent closures and session stores can be verified to the same fail-closed standard.

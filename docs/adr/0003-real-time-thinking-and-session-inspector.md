# ADR 0003: Session Inspector and Attention-First GPUI Panel (`mods/hotpot` & `mods/hotpot-session`)

- **Status**: accepted

The Herdr GPUI mods (`mods/hotpot` and `mods/hotpot-session`) combine `paseo-room`'s **Attention-First Runtime Panel UX** with `yetone/magpie`-inspired **inline model/thinking pills, preset profiles, and segmented context bars**.

## Decision

1. **Right-Hand Hotpot Panel (`mods/hotpot/`)**:
   - **Attention First, Colour Only the Exception**: Leads with a *Needs Attention* section (`NEEDS-HUMAN:`, `INCIDENT:`, stalled permissions, `Lead` past context rotation mark with **Replace Lead…** action) only when non-empty; collapses to a single muted `All quiet` line otherwise. Healthy states draw no loud badges.
   - **1:1 Project Seats with Inline Pickers & Preset Profiles**: Displays the `Project`'s dedicated `Supervisor`, `Lead`, and active `Peers`. Each seat card includes an inline `agent/model · thinking` pill picker (persisting atomically to `~/.hotpot/`) and a header preset switcher (*Deep Architecture*, *Fast Iteration*, *Budget*).
   - **Segmented Context Bar with Rotation Tick**: Shows each seat's context window broken down by `Cache Read`, `Fresh Input`, and `Output`, with a visible tick at the role's rotation/compaction mark (`30%` report / `50%` compact).
   - **Master–Detail Assignments**: Lists open assignments first and settled assignments newest-first with relative timestamps (`14m ago`), pushing into a detail view (`Brief`, `Peer`, `Candidate & Gate Evidence`, `History`).

2. **Center-Stage Session Inspector (`mods/hotpot-session/`)**:
   - Clicking any `Supervisor`, `Lead`, or `Peer` opens its structured conversation stream extracted from the agent's on-disk `.jsonl` session store under `~/.hotpot/roles/<agent>/<role>/` (`💭 Thinking`, `🔧 Tool Calls`, `📩 Attention / Assignment Notices`, and live prompt input via `hc send`), with a 1-click toggle (`Alt+T`) to focus the live `hc` PTY terminal.

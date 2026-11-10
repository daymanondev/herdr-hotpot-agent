//! SLP State Machine: 4-Part Contract validation, Ledger projection, and Role-scoped MCP tools.

use crate::guard::Role;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const REQUIRED_CONTRACT_SECTIONS: &[&str] = &[
    "1. Bounded Outcome",
    "2. Contract Boundary & Regression Invariants",
    "3. Excluded / Deferred",
    "4. Verification",
];

/// Segmented context-window occupancy of one seat (spec: "Render segmented context
/// bars (`Cache Read` / `Fresh Input` / `Output`) with role rotation ticks"): the
/// three segments, their total, and the rotation-tick flag set once the total
/// reaches the spec's rotation mark (>= 150k tokens).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSegments {
    pub cache_read: u64,
    pub fresh_input: u64,
    pub output: u64,
    pub total: u64,
    pub rotation_tick: bool,
}

impl ContextSegments {
    /// Segments plus their total and rotation-tick flag for a rotation mark of
    /// `rotation_mark_tokens` (the tick applies from the mark, inclusive).
    pub fn from_parts(cache_read: u64, fresh_input: u64, output: u64, rotation_mark_tokens: u64) -> Self {
        let total = cache_read + fresh_input + output;
        Self {
            cache_read,
            fresh_input,
            output,
            total,
            rotation_tick: total >= rotation_mark_tokens,
        }
    }

    /// The all-zero, no-tick segments reported for a seat with no usage events.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// One live `Seat` as projected by `hotpot status` (see [`crate::status`]). The
/// `provider` / `context_tokens` aliases keep the current `mods/hotpot` cards
/// rendering until tickets 13/14 rebuild them against this contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeatStatus {
    /// The seat's correlation id (the `hotpot.correlation` label).
    pub id: String,
    pub role: String,
    pub title: String,
    /// The seat's agent family (`codex` | `claude` | `pi`).
    pub agent: String,
    /// Compat alias of `agent` (read by `mods/hotpot/main.js` until 13/14).
    pub provider: String,
    /// From the dispatch record or the role profile defaults in Hotpot Home.
    pub model: Option<String>,
    /// The per-brief thinking effort (peer) or the role profile default.
    pub thinking: Option<String>,
    /// The `hc` terminal status; `offline` when the seat has no live terminal.
    pub status: String,
    pub terminal: Option<String>,
    /// The assignment a Peer seat is bound to.
    pub assignment: Option<String>,
    pub context: ContextSegments,
    /// Compat alias of `context.total` (read by `mods/hotpot/main.js` until 13/14).
    pub context_tokens: u64,
    pub activity: String,
}

/// One `Assignment` of the master list `hotpot status` serves (its `status`,
/// `agent_id`, `provider`, and `context_tokens` fields are compat aliases kept
/// for `mods/hotpot/main.js` until tickets 13/14 rebuild the task cards).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatus {
    pub id: String,
    pub state: String,
    pub disposition: String,
    pub verdict: Option<String>,
    /// The handback cycle number: 1 before the first handback, +1 per rework.
    pub attempt: u32,
    /// The assignment's outcome statement.
    pub summary: String,
    pub correlation: Option<String>,
    /// Compat alias of `correlation` (read by `mods/hotpot/main.js` until 13/14).
    pub agent_id: Option<String>,
    pub agent: Option<String>,
    /// Compat alias of `agent` (read by `mods/hotpot/main.js` until 13/14).
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub workspace: Option<String>,
    pub terminal: Option<String>,
    /// Compat alias of `state` (the live mod's spinner slot) until 13/14.
    pub status: String,
    pub context: ContextSegments,
    /// Compat alias of `context.total` (read by `mods/hotpot/main.js` until 13/14).
    pub context_tokens: u64,
    pub activity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxEntry {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub summary: String,
}

/// The `hotpot status` projection: the Project's seats, assignment master list,
/// due Attention Letters, and mailbox, rebuilt from the runtime ledger plus the
/// live `hc` terminal labels on every call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotpotStatusSnapshot {
    pub project: String,
    pub workspace: String,
    pub mode: String,
    /// The runtime directory under Hotpot Home this projection read
    /// (`<HOTPOT_HOME>/runtime/v1`), when it exists — the path the Herdr mods
    /// subscribe to with `watchFolder` for event-driven refreshes (spec
    /// "Implementation Decisions → Single Native Binary + Two GPUI Mods";
    /// ADR 0001). `None` while no runtime state exists yet, so a mod never
    /// subscribes to a missing directory and keeps its timer fallback.
    #[serde(rename = "runtimeDir")]
    pub runtime_dir: Option<String>,
    #[serde(rename = "generatedAt")]
    pub generated_at: u64,
    pub seats: Vec<SeatStatus>,
    pub tasks: Vec<TaskStatus>,
    /// The currently due Attention Letters (ticket 11), keyed by their stable
    /// `att_<id>` keys; served read-only so polling never consumes a batch.
    pub attention: Vec<Value>,
    pub mailbox: Vec<MailboxEntry>,
}

/// Mechanically validates that a Lead-authored SLP Contract description contains all 4 canonical sections.
pub fn validate_contract_description(description: &str) -> Result<(), String> {
    if description.chars().count() > 2500 {
        return Err("Contract description exceeds 2,500-character ceiling".to_string());
    }
    for section in REQUIRED_CONTRACT_SECTIONS {
        if !description.contains(section) {
            return Err(format!(
                "invalid-contract: missing required section `{section}`"
            ));
        }
    }
    Ok(())
}

/// Returns the strictly role-scoped MCP tool definitions for `role`, using the
/// canonical Runtime Coordinator tool names (spec: "Role-Asymmetric MCP Surface").
/// The single source of truth lives in [`crate::runtime::role_tool_defs`].
pub fn role_scoped_mcp_tools(role: Role) -> Vec<Value> {
    crate::runtime::role_tool_defs(role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_validation_enforces_four_canonical_sections() {
        let bad = "Just fix the login bug and run tests.";
        assert!(validate_contract_description(bad).is_err());

        let good = "\
1. Bounded Outcome\nImplement token parser.\n\
2. Contract Boundary & Regression Invariants\nKeep existing CLI intact.\n\
3. Excluded / Deferred\nUI changes deferred.\n\
4. Verification\ncargo test";
        assert!(validate_contract_description(good).is_ok());
    }

    #[test]
    fn peer_mcp_slice_is_strictly_ask_and_handoff() {
        let peer_tools = role_scoped_mcp_tools(Role::Agent);
        let names: Vec<&str> = peer_tools
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, vec!["ask", "handoff"]);
    }
}

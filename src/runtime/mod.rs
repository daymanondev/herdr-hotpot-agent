//! Runtime Coordinator Layer: the role-asymmetric `hotpot mcp` stdio JSON-RPC 2.0
//! server backed by an append-only event log under `<HOTPOT_HOME>/runtime/v1/`.
//!
//! This slice delivers the exact per-role tool surface, the Lead `Assignment`
//! lifecycle (`Draft -> Dispatched -> Asking | HandedBack | Stopped`; terminal
//! states -> `Closed`), verified `baseCommit`, the `thinking` effort refusal, and
//! Writer Leases for writable assignments: the exclusive in-checkout lease after
//! clean-HEAD verification, plus up to three scope-disjoint isolated worktrees
//! provisioned with a warm build cache (see [`leases`]). The Peer surface is
//! strictly `ask`/`handoff` with identity, assignment, and workspace derived
//! server-side from the session correlation (never from tool arguments), and
//! every writable `handoff` validated against live Git `baseCommit..Candidate`
//! evidence (see [`evidence`]). `gate_run` re-executes the Gate against the
//! handed-back candidate in a detached background process whose completion is
//! recorded as a ledger event with a masked red tail (see [`gate`] and
//! [`masking`]); nothing is ever auto-accepted on a green gate. The Supervisor
//! surface detects and batch-delivers rate-limited Attention Letters, honors
//! `attention_feedback`, routes Human directives with `message_lead`, and
//! replaces a context-rotated Lead at a verified quiet point through the
//! `lead_replace_prepare` → review → `lead_replace_confirm` sequence (see
//! [`attention`]).

pub mod attention;
pub mod evidence;
pub mod gate;
pub mod ledger;
pub mod leases;
pub mod masking;
pub mod replacement;

pub use ledger::{Ledger, project_slug, sha1_hex};
pub use leases::{IN_CHECKOUT_EXCLUSIVE, WriterLease};

use crate::guard::Role;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Canonical `Role` name on the MCP surface (`guard::Role::Agent` is a `Peer` here).
pub fn mcp_role_name(role: Role) -> &'static str {
    match role {
        Role::Supervisor => "supervisor",
        Role::Lead => "lead",
        Role::Agent => "peer",
    }
}

/// Spec: "Implementation Decisions -> Role-Asymmetric MCP Surface (`hotpot mcp`)".
pub const SUPERVISOR_TOOLS: &[&str] = &[
    "room_status",
    "runtime_findings",
    "message_lead",
    "attention_feedback",
    "lead_replace_prepare",
    "lead_replace_confirm",
    "lead_replace_cancel",
];

pub const LEAD_TOOLS: &[&str] = &[
    "assignment_create",
    "assignment_dispatch",
    "assignment_status",
    "assignment_answer",
    "assignment_accept",
    "assignment_close",
    "gate_run",
    "workspace_close",
    "lease_reclaim",
];

pub const PEER_TOOLS: &[&str] = &["ask", "handoff"];

/// The exact tool-name set a role may see in `tools/list` (and call).
pub fn role_tool_names(role: Role) -> &'static [&'static str] {
    match role {
        Role::Supervisor => SUPERVISOR_TOOLS,
        Role::Lead => LEAD_TOOLS,
        Role::Agent => PEER_TOOLS,
    }
}

/// Tool definitions (name + description) served by `tools/list` for `role`.
pub fn role_tool_defs(role: Role) -> Vec<Value> {
    let descriptions: &[(&str, &str)] = match role {
        Role::Supervisor => &[
            (
                "room_status",
                "Read the live room snapshot for this Project (seats, assignments, attention).",
            ),
            (
                "runtime_findings",
                "Read verified runtime findings for this Project.",
            ),
            (
                "message_lead",
                "Route a Human directive to the Project's Lead.",
            ),
            (
                "attention_feedback",
                "Rate a noisy Attention Letter to suppress or re-raise it.",
            ),
            (
                "lead_replace_prepare",
                "Request a structured handoff from the Lead at a verified quiet point.",
            ),
            (
                "lead_replace_confirm",
                "Archive the old Lead and launch a successor seeded with the reviewed handoff.",
            ),
            (
                "lead_replace_cancel",
                "Abort an in-flight Lead replacement.",
            ),
        ],
        Role::Lead => &[
            (
                "assignment_create",
                "Create a Draft Assignment with outcome, Disposition, verified baseCommit, optional Write Scope / serialOnly globs, Gate command, and thinking effort.",
            ),
            (
                "assignment_dispatch",
                "Dispatch a Draft Assignment to a Peer, registering its correlation -> (project, assignment, workspace) binding; optionally request the seat's agent family and model (e.g. a Reviewer from a different family).",
            ),
            (
                "assignment_status",
                "Read the current state and all recorded fields of an Assignment.",
            ),
            (
                "assignment_answer",
                "Answer a Peer question on an Asking Assignment, returning it to Dispatched.",
            ),
            (
                "assignment_accept",
                "Accept a handed-back Assignment after integrating its Candidate.",
            ),
            (
                "assignment_close",
                "Stop in-flight work or finalize a terminal Assignment as Closed.",
            ),
            (
                "gate_run",
                "Re-execute the Assignment Gate against its Candidate in a detached background process; the completion verdict (green, red with a masked tail, or uncertain) is recorded as a ledger event.",
            ),
            (
                "workspace_close",
                "Close an Assignment workspace and release isolated worktree resources.",
            ),
            ("lease_reclaim", "Reclaim an expired Writer Lease."),
        ],
        Role::Agent => &[
            (
                "ask",
                "Raise a question or a structured Challenge Signal (REOPEN_REQUEST, DEPENDENCY_REQUEST, BLOCKED) to the Lead for the current Assignment; identity, assignment, and workspace are derived server-side from the session correlation.",
            ),
            (
                "handoff",
                "Submit finished work for the current Assignment, validated against live Git baseCommit..Candidate evidence; identity, assignment, and workspace are derived server-side from the session correlation.",
            ),
        ],
    };
    descriptions
        .iter()
        .map(|(name, description)| json!({"name": name, "description": description}))
        .collect()
}

/// A JSON-RPC 2.0 error returned by a tool call or the dispatcher itself.
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
        }
    }
}

fn rpc_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error_response(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Runs the stdio JSON-RPC 2.0 server loop: one response line per request line.
/// `correlation` is the server-side session identity pin a Peer seat derives its
/// assignment binding from (`--correlation` / `HOTPOT_CORRELATION` at launch).
pub fn serve_mcp(
    role: Role,
    project_root: &Path,
    state_dir: &Path,
    correlation: Option<&str>,
) -> Result<(), String> {
    let ledger = Ledger::open(state_dir, project_root).map_err(|e| e.to_string())?;
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(raw) = line else { break };
        if raw.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(&ledger, role, project_root, state_dir, correlation, &raw) {
            let _ = writeln!(stdout, "{reply}");
            let _ = stdout.flush();
        }
    }
    Ok(())
}

/// Handles one raw request line; returns `None` for JSON-RPC notifications (no `id`).
fn handle_line(
    ledger: &Ledger,
    role: Role,
    project_root: &Path,
    state_dir: &Path,
    correlation: Option<&str>,
    raw: &str,
) -> Option<Value> {
    let req: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => {
            return Some(rpc_error_response(
                Value::Null,
                -32700,
                "parse error: request line is not valid JSON",
            ));
        }
    };
    // A request without an `id` key is a notification: never answer it.
    let id = req.get("id").cloned()?;
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => Some(rpc_response(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "hotpot", "version": env!("CARGO_PKG_VERSION") }
            }),
        )),
        "tools/list" => Some(rpc_response(id, json!({ "tools": role_tool_defs(role) }))),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                return Some(rpc_error_response(
                    id,
                    -32602,
                    "tools/call requires a tool name",
                ));
            }
            if !role_tool_names(role).contains(&name) {
                return Some(rpc_error_response(
                    id,
                    -32602,
                    &format!(
                        "unknown tool `{name}` for role `{}` (the MCP surface is role-asymmetric)",
                        mcp_role_name(role)
                    ),
                ));
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(ledger, project_root, state_dir, correlation, name, &arguments) {
                Ok(result) => Some(rpc_response(id, result)),
                Err(e) => Some(rpc_error_response(id, e.code, &e.message)),
            }
        }
        "" => Some(rpc_error_response(
            id,
            -32600,
            "invalid request: missing method",
        )),
        other => Some(rpc_error_response(
            id,
            -32601,
            &format!("method not found: {other}"),
        )),
    }
}

/// Dispatches a role-fenced `tools/call` to its handler.
fn call_tool(
    ledger: &Ledger,
    project_root: &Path,
    state_dir: &Path,
    correlation: Option<&str>,
    tool: &str,
    args: &Value,
) -> Result<Value, RpcError> {
    match tool {
        "assignment_create" => assignment_create(ledger, project_root, args),
        "assignment_dispatch" => assignment_dispatch(ledger, project_root, args),
        "assignment_status" => assignment_status(ledger, args),
        "assignment_answer" => assignment_answer(ledger, args),
        "assignment_accept" => assignment_accept(ledger, args),
        "assignment_close" => assignment_close(ledger, args),
        "gate_run" => gate_run(ledger, project_root, state_dir, args),
        "workspace_close" => workspace_close(ledger, project_root, args),
        "lease_reclaim" => lease_reclaim(ledger, args),
        "runtime_findings" => runtime_findings(ledger),
        "room_status" => room_status(ledger),
        "attention_feedback" => attention_feedback(ledger, args),
        "message_lead" => message_lead(ledger, state_dir, project_root, args),
        "lead_replace_prepare" => lead_replace_prepare(ledger),
        "lead_replace_confirm" => lead_replace_confirm(ledger, state_dir, project_root, args),
        "lead_replace_cancel" => lead_replace_cancel(ledger, args),
        "ask" => peer_ask(ledger, correlation, args),
        "handoff" => peer_handoff(ledger, correlation, args),
        _ => Err(RpcError::not_implemented(format!(
            "`{tool}` semantics land with the assignment lifecycle slice"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Assignment projection (rebuilt from the ledger; no second database)
// ---------------------------------------------------------------------------

/// The `AssignmentState` names of the spec's state machine.
pub const ASSIGNMENT_STATES: &[&str] = &[
    "Draft",
    "Dispatched",
    "Asking",
    "HandedBack",
    "Stopped",
    "GateRunning",
    "Accepted",
    "ReworkRequested",
    "Rejected",
    "Abandoned",
    "Closed",
];

/// Permitted `Disposition` stances for a `Peer` brief.
pub const DISPOSITIONS: &[&str] = &["Engineer", "Architect", "Reviewer", "Scout"];

/// Permitted per-brief thinking efforts; `ultra`/`ultracode` are refused.
pub const THINKING_EFFORTS: &[&str] = &["low", "medium", "high"];
pub const REFUSED_THINKING_EFFORTS: &[&str] = &["ultra", "ultracode"];

/// The verified seat agent families a dispatch may request (a Reviewer may be
/// dispatched on a different family than the engineer whose candidate it
/// reviews); anything else is refused.
pub const AGENT_FAMILIES: &[&str] = &["codex", "claude", "pi"];

/// One `Assignment` (`asg_<id>`) as projected from the ledger.
#[derive(Debug, Clone)]
pub struct Assignment {
    pub id: String,
    pub state: String,
    pub outcome: String,
    pub disposition: String,
    pub base_commit: String,
    pub write_scope: Vec<String>,
    pub serial_only: Vec<String>,
    pub gate: String,
    pub thinking: Option<String>,
    pub thinking_reason: Option<String>,
    pub correlation: Option<String>,
    pub workspace: Option<String>,
    /// The dispatched seat's agent family (`codex`|`claude`|`pi`), when Lead
    /// requested one (a Reviewer may run on a different family than the
    /// engineer whose candidate it reviews).
    pub agent: Option<String>,
    /// The dispatched seat's free-form model id, when Lead requested one.
    pub model: Option<String>,
    /// The Lead's latest `assignment_answered` text, visible to the Peer seat.
    pub last_answer: Option<String>,
    /// The verified `Candidate` commit of the latest accepted handback.
    pub candidate: Option<String>,
    /// Unix-seconds timestamp of the handback that is still unread by the Lead
    /// (feeds the unread-handback Attention Letter).
    pub handed_back_at: Option<u64>,
    /// Verdict of the latest completed `gate_run` (`green`|`red`|`uncertain`).
    pub gate_verdict: Option<String>,
    /// Path of the masked red tail of the latest completed `gate_run`.
    pub gate_tail: Option<String>,
    /// Uncertain reason (`timeout`, `spawn-failed`, ...) of the latest gate run.
    pub gate_reason: Option<String>,
}

impl Assignment {
    /// The assignment as served by `assignment_status` (and `hotpot export`).
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "state": self.state,
            "outcome": self.outcome,
            "disposition": self.disposition,
            "baseCommit": self.base_commit,
            "writeScope": self.write_scope,
            "serialOnly": self.serial_only,
            "gate": self.gate,
            "thinking": self.thinking,
            "thinkingReason": self.thinking_reason,
            "correlation": self.correlation,
            "workspace": self.workspace,
            "agent": self.agent,
            "model": self.model,
            "lastAnswer": self.last_answer,
            "candidate": self.candidate,
            "handedBackAt": self.handed_back_at,
            "gateVerdict": self.gate_verdict,
            "gateTail": self.gate_tail,
            "gateReason": self.gate_reason,
        })
    }
}

/// All assignments and active Writer Leases of one project ledger, rebuilt by
/// replaying the event log.
#[derive(Debug, Default)]
pub struct RuntimeState {
    assignments: std::collections::BTreeMap<String, Assignment>,
    leases: std::collections::BTreeMap<String, WriterLease>,
}

impl RuntimeState {
    pub fn load(ledger: &Ledger) -> Self {
        Self::from_events(&ledger.read_events())
    }

    /// Rebuilds the projection from raw ledger events (used by the evidence
    /// recorder, which reads ledgers without a project root).
    pub fn from_events(events: &[Value]) -> Self {
        let mut state = Self::default();
        for event in events {
            state.apply(event);
        }
        state
    }

    pub fn assignment(&self, id: &str) -> Option<&Assignment> {
        self.assignments.get(id)
    }

    /// Every projected assignment in id order (used by `hotpot export`).
    pub fn assignments(&self) -> Vec<&Assignment> {
        self.assignments.values().collect()
    }

    /// The active Writer Lease held for `assignment`, if any.
    pub fn lease_for(&self, assignment: &str) -> Option<&WriterLease> {
        self.leases.get(assignment)
    }

    /// Every currently held Writer Lease (release order = acquisition order).
    pub fn active_leases(&self) -> Vec<&WriterLease> {
        self.leases.values().collect()
    }

    fn apply(&mut self, event: &Value) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let id = event
            .get("assignment")
            .and_then(Value::as_str)
            .unwrap_or("");
        if id.is_empty() {
            return;
        }
        match kind {
            "assignment_created" => {
                self.assignments.insert(
                    id.to_string(),
                    Assignment {
                        id: id.to_string(),
                        state: "Draft".to_string(),
                        outcome: str_field(event, "outcome"),
                        disposition: str_field(event, "disposition"),
                        base_commit: str_field(event, "baseCommit"),
                        write_scope: str_array_field(event, "writeScope"),
                        serial_only: str_array_field(event, "serialOnly"),
                        gate: str_field(event, "gate"),
                        thinking: opt_str_field(event, "thinking"),
                        thinking_reason: opt_str_field(event, "thinkingReason"),
                        correlation: None,
                        workspace: None,
                        agent: None,
                        model: None,
                        last_answer: None,
                        candidate: None,
                        handed_back_at: None,
                        gate_verdict: None,
                        gate_tail: None,
                        gate_reason: None,
                    },
                );
            }
            "writer_lease_acquired" => {
                self.leases.insert(
                    id.to_string(),
                    WriterLease {
                        assignment: id.to_string(),
                        mode: str_field(event, "mode"),
                        worktree: opt_str_field(event, "worktree"),
                        cargo_target_dir: opt_str_field(event, "cargoTargetDir"),
                        worktree_include: str_array_field(event, "worktreeInclude"),
                    },
                );
            }
            "writer_lease_released" | "writer_lease_reclaimed" => {
                self.leases.remove(id);
            }
            "assignment_dispatched" => {
                if let Some(a) = self.assignments.get_mut(id) {
                    a.correlation = opt_str_field(event, "correlation");
                    a.workspace = opt_str_field(event, "workspace");
                    a.agent = opt_str_field(event, "agent");
                    a.model = opt_str_field(event, "model");
                    if let Some(next) = transition(&a.state, kind) {
                        a.state = next.to_string();
                    }
                }
            }
            "assignment_answered" => {
                if let Some(a) = self.assignments.get_mut(id) {
                    a.last_answer = opt_str_field(event, "answer");
                    if let Some(next) = transition(&a.state, kind) {
                        a.state = next.to_string();
                    }
                }
            }
            "assignment_handed_back" => {
                if let Some(a) = self.assignments.get_mut(id) {
                    a.candidate = opt_str_field(event, "candidate");
                    a.handed_back_at = event.get("ts").and_then(Value::as_u64);
                    if let Some(next) = transition(&a.state, kind) {
                        a.state = next.to_string();
                    }
                }
            }
            "gate_started" => {
                // A new gate run supersedes any recorded verdict while it runs.
                if let Some(a) = self.assignments.get_mut(id) {
                    a.gate_verdict = None;
                    a.gate_tail = None;
                    a.gate_reason = None;
                    if let Some(next) = transition(&a.state, kind) {
                        a.state = next.to_string();
                    }
                }
            }
            "gate_completed" => {
                if let Some(a) = self.assignments.get_mut(id) {
                    a.gate_verdict = opt_str_field(event, "verdict");
                    a.gate_tail = opt_str_field(event, "tail");
                    a.gate_reason = opt_str_field(event, "reason");
                    if let Some(next) = transition(&a.state, kind) {
                        a.state = next.to_string();
                    }
                }
            }
            _ => {
                if let Some(a) = self.assignments.get_mut(id)
                    && let Some(next) = transition(&a.state, kind)
                {
                    a.state = next.to_string();
                }
            }
        }
    }
}

/// The spec's `AssignmentState` machine:
/// `Draft -> Dispatched -> Asking | HandedBack | Stopped`;
/// `HandedBack -> GateRunning (optional) -> Accepted | ReworkRequested | Rejected | Abandoned`;
/// `Accepted | Rejected | Abandoned -> Closed`.
/// A completed gate returns the assignment to `HandedBack` carrying its
/// verdict — acceptance stays an explicit Lead transition (`assignment_accept`
/// also legal straight out of `GateRunning`), so nothing is auto-accepted.
/// Returns the next state when `kind` is a legal transition from `state`.
fn transition(state: &str, kind: &str) -> Option<&'static str> {
    match (state, kind) {
        ("Draft", "assignment_dispatched") => Some("Dispatched"),
        ("Dispatched", "assignment_asked") => Some("Asking"),
        ("Asking", "assignment_answered") => Some("Dispatched"),
        ("Dispatched", "assignment_handed_back")
        | ("Asking", "assignment_handed_back")
        | ("ReworkRequested", "assignment_handed_back") => Some("HandedBack"),
        ("HandedBack", "gate_started") => Some("GateRunning"),
        ("GateRunning", "gate_completed") => Some("HandedBack"),
        ("HandedBack", "assignment_accepted") | ("GateRunning", "assignment_accepted") => {
            Some("Accepted")
        }
        ("HandedBack", "assignment_rework_requested")
        | ("GateRunning", "assignment_rework_requested") => Some("ReworkRequested"),
        ("HandedBack", "assignment_rejected") | ("GateRunning", "assignment_rejected") => {
            Some("Rejected")
        }
        ("Draft", "assignment_stopped")
        | ("Dispatched", "assignment_stopped")
        | ("Asking", "assignment_stopped") => Some("Stopped"),
        ("HandedBack", "assignment_abandoned") | ("GateRunning", "assignment_abandoned") => {
            Some("Abandoned")
        }
        ("Accepted", "assignment_closed")
        | ("Rejected", "assignment_closed")
        | ("Abandoned", "assignment_closed") => Some("Closed"),
        _ => None,
    }
}

fn str_field(event: &Value, key: &str) -> String {
    event
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn opt_str_field(event: &Value, key: &str) -> Option<String> {
    event
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

fn str_array_field(event: &Value, key: &str) -> Vec<String> {
    event
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn arg_str(args: &Value, tool: &str, key: &str) -> Result<String, RpcError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            RpcError::invalid_params(format!("{tool}: `{key}` is required and must be non-empty"))
        })
}

fn arg_opt_str_array(args: &Value, tool: &str, key: &str) -> Result<Option<Vec<String>>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => Ok(Some(
            items
                .iter()
                .map(|v| {
                    v.as_str().map(|s| s.to_string()).ok_or_else(|| {
                        RpcError::invalid_params(format!(
                            "{tool}: `{key}` must be an array of glob strings"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Some(_) => Err(RpcError::invalid_params(format!(
            "{tool}: `{key}` must be an array of glob strings"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

/// Verifies `commit` resolves to an existing commit object in the project git repo.
fn verify_base_commit(project_root: &Path, commit: &str) -> Result<(), RpcError> {
    let spec = format!("{commit}^{{commit}}");
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["cat-file", "-e", &spec])
        .output()
        .map_err(|e| RpcError::internal(format!("failed to invoke git: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!(
            "assignment_create: baseCommit `{commit}` does not exist in the project git repository ({})",
            project_root.display()
        )))
    }
}

fn assignment_create(
    ledger: &Ledger,
    project_root: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_create";

    let outcome = arg_str(args, TOOL, "outcome")?;
    let disposition_raw = arg_str(args, TOOL, "disposition")?;
    let disposition = DISPOSITIONS
        .iter()
        .find(|d| d.eq_ignore_ascii_case(&disposition_raw))
        .ok_or_else(|| {
            RpcError::invalid_params(format!(
                "{TOOL}: invalid Disposition `{disposition_raw}`: permitted Dispositions are {}",
                DISPOSITIONS.join("|")
            ))
        })?;
    let base_commit = arg_str(args, TOOL, "baseCommit")?;
    verify_base_commit(project_root, &base_commit)?;
    let gate = arg_str(args, TOOL, "gate")?;
    let write_scope = arg_opt_str_array(args, TOOL, "writeScope")?.unwrap_or_default();
    let serial_only = arg_opt_str_array(args, TOOL, "serialOnly")?.unwrap_or_default();

    let thinking = match args.get("thinking") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let effort = s.trim();
            if REFUSED_THINKING_EFFORTS.contains(&effort) {
                return Err(RpcError::invalid_params(format!(
                    "{TOOL}: thinking effort `{effort}` refused: ultra/ultracode are not permitted brief efforts; use low|medium|high"
                )));
            }
            if THINKING_EFFORTS.contains(&effort) {
                Some(effort.to_string())
            } else {
                return Err(RpcError::invalid_params(format!(
                    "{TOOL}: invalid thinking effort `{effort}`: permitted efforts are low|medium|high"
                )));
            }
        }
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `thinking` must be a string effort (low|medium|high)"
            )));
        }
    };
    let thinking_reason = match &thinking {
        Some(_) => Some(arg_str(args, TOOL, "thinkingReason")?),
        None => args
            .get("thinkingReason")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    };

    // Deterministic, replay-stable id: the Nth assignment created in this ledger.
    let ordinal = ledger
        .read_events()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("assignment_created"))
        .count()
        + 1;
    let id = format!("asg_{ordinal:03}");

    let mut event = json!({
        "type": "assignment_created",
        "assignment": id,
        "outcome": outcome,
        "disposition": disposition,
        "baseCommit": base_commit,
        "writeScope": write_scope,
        "serialOnly": serial_only,
        "gate": gate,
    });
    if let Some(t) = &thinking {
        event["thinking"] = json!(t);
    }
    if let Some(r) = &thinking_reason {
        event["thinkingReason"] = json!(r);
    }
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let state = RuntimeState::load(ledger);
    Ok(state
        .assignment(&id)
        .expect("just-created assignment replays")
        .to_json())
}

fn assignment_status(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_status";
    let id = arg_str(args, TOOL, "assignment")?;
    RuntimeState::load(ledger)
        .assignment(&id)
        .map(|a| a.to_json())
        .ok_or_else(|| RpcError::invalid_params(format!("{TOOL}: unknown assignment `{id}`")))
}

/// Loads the current assignment projection or fails with a JSON-RPC error.
fn require_assignment(ledger: &Ledger, tool: &str, id: &str) -> Result<Assignment, RpcError> {
    RuntimeState::load(ledger)
        .assignment(id)
        .cloned()
        .ok_or_else(|| RpcError::invalid_params(format!("{tool}: unknown assignment `{id}`")))
}

/// Fails unless the assignment currently sits in one of `allowed` states.
fn require_state(
    tool: &str,
    action: &str,
    asg: &Assignment,
    allowed: &[&str],
) -> Result<(), RpcError> {
    if allowed.contains(&asg.state.as_str()) {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!(
            "{tool}: illegal transition: assignment `{}` is `{}`; {action} requires {}",
            asg.id,
            asg.state,
            allowed.join("|")
        )))
    }
}

/// Appends one lifecycle event and returns the reloaded assignment projection.
fn append_and_reload(ledger: &Ledger, tool: &str, event: Value) -> Result<Value, RpcError> {
    let id = event
        .get("assignment")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{tool}: {e}")))?;
    let state = RuntimeState::load(ledger);
    Ok(state
        .assignment(&id)
        .expect("assignment exists before its lifecycle event")
        .to_json())
}

fn assignment_dispatch(
    ledger: &Ledger,
    project_root: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_dispatch";
    let id = arg_str(args, TOOL, "assignment")?;
    let asg = require_assignment(ledger, TOOL, &id)?;
    require_state(TOOL, "dispatch", &asg, &["Draft"])?;

    // The optional seat family / model this peer runs on (e.g. a Reviewer from
    // a different family than the engineer whose candidate it reviews).
    let agent = match args.get("agent") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let requested = s.trim();
            let family = AGENT_FAMILIES
                .iter()
                .find(|f| f.eq_ignore_ascii_case(requested))
                .ok_or_else(|| {
                    RpcError::invalid_params(format!(
                        "{TOOL}: invalid agent family `{requested}`: the verified seat families are {}",
                        AGENT_FAMILIES.join("|")
                    ))
                })?;
            Some(family.to_string())
        }
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `agent` must be a seat family string ({})",
                AGENT_FAMILIES.join("|")
            )))
        }
    };
    let model = match args.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let requested = s.trim();
            if requested.is_empty() {
                return Err(RpcError::invalid_params(format!(
                    "{TOOL}: `model` must be a non-empty model id string"
                )));
            }
            Some(requested.to_string())
        }
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `model` must be a non-empty model id string"
            )))
        }
    };

    // Writability is declared by a non-empty Write Scope: read-only assignments
    // (e.g. a Reviewer) acquire no Writer Lease and never touch the checkout.
    let writable = !asg.write_scope.is_empty();
    let isolation = leases::parse_isolation(args)?;
    let state = RuntimeState::load(ledger);
    let mut provision: Option<leases::WorktreeProvision> = None;
    if writable {
        match isolation {
            leases::Isolation::Checkout => {
                // The exclusive checkout lease blocks writes from all other
                // seats — including Lead itself — until it is released (spec
                // user story 20).
                if let Some(holder) = state.active_leases().first() {
                    return Err(RpcError::invalid_params(format!(
                        "{TOOL}: writer_exclusive: the checkout writer lease is held by `{}` (mode `{}`); writable dispatch is refused for all seats (including Lead) until the lease is released",
                        holder.assignment, holder.mode
                    )));
                }
                leases::verify_checkout_preconditions(project_root, &asg.base_commit)?;
            }
            leases::Isolation::Worktree => {
                // Isolated worktree writers: at most LEASE_CAP concurrent, with
                // pairwise-disjoint Write Scope and serialOnly globs against the
                // active worktree leases (spec user story 21).
                let peers: Vec<(String, Vec<String>, Vec<String>)> = state
                    .active_leases()
                    .iter()
                    .filter(|l| l.mode == leases::ISOLATED_WORKTREE)
                    .map(|l| {
                        let peer = state.assignment(&l.assignment);
                        (
                            l.assignment.clone(),
                            peer.map(|a| a.write_scope.clone()).unwrap_or_default(),
                            peer.map(|a| a.serial_only.clone()).unwrap_or_default(),
                        )
                    })
                    .collect();
                if peers.len() >= leases::LEASE_CAP {
                    let holders = peers
                        .iter()
                        .map(|(id, ..)| format!("`{id}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(RpcError::invalid_params(format!(
                        "{TOOL}: lease_cap: the isolated worktree lease cap of {} concurrent writers is reached (held by {holders}); release a lease before dispatching another isolated writer",
                        leases::LEASE_CAP
                    )));
                }
                leases::check_scope_disjointness(
                    TOOL,
                    &asg.write_scope,
                    &asg.serial_only,
                    &peers,
                )?;
                provision = Some(leases::provision_worktree(
                    project_root,
                    ledger.project_dir(),
                    &id,
                    &asg.base_commit,
                )?);
            }
        }
    }

    // The correlation -> (project, assignment, workspace) binding is what the peer
    // identity slice resolves server-side; peers never supply their own ids.
    let ordinal = ledger
        .read_events()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("assignment_dispatched"))
        .count()
        + 1;
    let correlation = args
        .get("correlation")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("corr_{ordinal:03}"));
    // An isolated writer's workspace is its provisioned worktree; every other
    // dispatch works in the project checkout.
    let workspace = match &provision {
        Some(p) => p.worktree.to_string_lossy().to_string(),
        None => args
            .get("workspace")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                std::fs::canonicalize(project_root)
                    .unwrap_or_else(|_| project_root.to_path_buf())
                    .to_string_lossy()
                    .to_string()
            }),
    };

    let mut dispatch_event = json!({
        "type": "assignment_dispatched",
        "assignment": id,
        "correlation": correlation,
        "workspace": workspace,
    });
    if let Some(agent) = &agent {
        dispatch_event["agent"] = json!(agent);
    }
    if let Some(model) = &model {
        dispatch_event["model"] = json!(model);
    }
    ledger
        .append(dispatch_event)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    if writable {
        let event = match &provision {
            Some(p) => leases::lease_acquired_event(
                &id,
                leases::ISOLATED_WORKTREE,
                Some(&p.worktree),
                Some(&p.cargo_target_dir),
                &p.worktree_include,
            ),
            None => leases::lease_acquired_event(
                &id,
                leases::IN_CHECKOUT_EXCLUSIVE,
                None,
                None,
                &[],
            ),
        };
        ledger
            .append(event)
            .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    }

    let state = RuntimeState::load(ledger);
    let mut result = state
        .assignment(&id)
        .expect("assignment exists before its lifecycle event")
        .to_json();
    if let Some(lease) = state.lease_for(&id) {
        result["lease"] = json!({
            "mode": lease.mode,
            "worktree": lease.worktree,
            "cargoTargetDir": lease.cargo_target_dir,
            "worktreeInclude": lease.worktree_include,
        });
    }
    Ok(result)
}

/// `workspace_close`: archive the assignment's workspace — release its Writer
/// Lease, clean/retain its isolated worktree per assignment state (a handed-back
/// candidate awaiting manual integration is retained, anything else is cleaned
/// up), and finalize a terminal disposition (`Accepted` | `Rejected` |
/// `Abandoned`) as `Closed` per the state machine. Read-only assignments hold
/// no workspace resources, so closing them only finalizes the state.
fn workspace_close(
    ledger: &Ledger,
    project_root: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "workspace_close";
    let id = arg_str(args, TOOL, "assignment")?;
    let state = RuntimeState::load(ledger);
    let lease = state.lease_for(&id).cloned();
    let asg = require_assignment(ledger, TOOL, &id)?;
    let terminal = ["Accepted", "Rejected", "Abandoned"].contains(&asg.state.as_str());
    if lease.is_none() && !terminal {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: no active writer lease for `{id}`; workspace_close releases a lease held by a dispatched assignment"
        )));
    }

    let action = if let Some(lease) = &lease
        && lease.mode == leases::ISOLATED_WORKTREE
        && let Some(worktree) = &lease.worktree
    {
        if ["HandedBack", "GateRunning"].contains(&asg.state.as_str()) {
            "retained"
        } else {
            leases::remove_worktree(project_root, Path::new(worktree))?;
            "removed"
        }
    } else {
        "none"
    };

    if lease.is_some() {
        ledger
            .append(json!({
                "type": "writer_lease_released",
                "assignment": id,
                "worktreeAction": action,
            }))
            .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    }
    // The peer workspace is archived; a terminal disposition becomes Closed.
    if terminal {
        ledger
            .append(json!({"type": "assignment_closed", "assignment": id}))
            .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    }
    let state = RuntimeState::load(ledger);
    let mut result = state
        .assignment(&id)
        .expect("assignment exists before its lifecycle event")
        .to_json();
    result["lease"] = json!({
        "released": lease.is_some(),
        "worktreeAction": action,
    });
    Ok(result)
}

/// `lease_reclaim`: Lead force-reclaims a stale Writer Lease, appending an audit
/// event and freeing the writer slot. The isolated worktree, if any, stays on
/// disk for Lead to integrate or discard deliberately.
fn lease_reclaim(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "lease_reclaim";
    let id = arg_str(args, TOOL, "assignment")?;
    let state = RuntimeState::load(ledger);
    let lease = state
        .lease_for(&id)
        .ok_or_else(|| {
            RpcError::invalid_params(format!(
                "{TOOL}: no active writer lease for `{id}`; lease_reclaim force-frees a held lease"
            ))
        })?
        .clone();

    ledger
        .append(json!({
            "type": "writer_lease_reclaimed",
            "assignment": id,
            "mode": lease.mode,
            "worktreeAction": "retained",
        }))
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    let state = RuntimeState::load(ledger);
    let mut result = state
        .assignment(&id)
        .expect("assignment exists before its lifecycle event")
        .to_json();
    result["lease"] = json!({
        "reclaimed": true,
        "mode": lease.mode,
        "worktree": lease.worktree,
        "worktreeAction": "retained",
    });
    Ok(result)
}

fn assignment_answer(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_answer";
    let id = arg_str(args, TOOL, "assignment")?;
    let answer = arg_str(args, TOOL, "answer")?;
    let asg = require_assignment(ledger, TOOL, &id)?;
    require_state(TOOL, "answering", &asg, &["Asking"])?;
    append_and_reload(
        ledger,
        TOOL,
        json!({"type": "assignment_answered", "assignment": id, "answer": answer}),
    )
}

fn assignment_accept(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_accept";
    let id = arg_str(args, TOOL, "assignment")?;
    let asg = require_assignment(ledger, TOOL, &id)?;
    require_state(TOOL, "accepting", &asg, &["HandedBack", "GateRunning"])?;
    let mut event = json!({"type": "assignment_accepted", "assignment": id});
    if let Some(note) = args.get("note").and_then(Value::as_str) {
        event["note"] = json!(note);
    }
    append_and_reload(ledger, TOOL, event)
}

fn assignment_close(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_close";
    let id = arg_str(args, TOOL, "assignment")?;
    let asg = require_assignment(ledger, TOOL, &id)?;

    // In-flight work is stopped; handed-back work is abandoned; a terminal
    // disposition (Accepted | Rejected | Abandoned) is finalized as Closed.
    let kind = match asg.state.as_str() {
        "Draft" | "Dispatched" | "Asking" => "assignment_stopped",
        "HandedBack" | "GateRunning" => "assignment_abandoned",
        "Accepted" | "Rejected" | "Abandoned" => "assignment_closed",
        "Closed" | "Stopped" => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: illegal transition: assignment `{id}` is already `{}`",
                asg.state
            )));
        }
        other => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: illegal transition: assignment `{id}` is `{other}`; it must reach a terminal disposition before closing"
            )));
        }
    };
    append_and_reload(ledger, TOOL, json!({"type": kind, "assignment": id}))
}

fn gate_run(
    ledger: &Ledger,
    project_root: &Path,
    state_dir: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "gate_run";
    let id = arg_str(args, TOOL, "assignment")?;
    let asg = require_assignment(ledger, TOOL, &id)?;
    require_state(TOOL, "starting the gate", &asg, &["HandedBack"])?;
    let timeout_ms = match args.get("timeoutMs") {
        None | Some(Value::Null) => gate::DEFAULT_TIMEOUT_MS,
        Some(Value::Number(n)) => n.as_u64().filter(|ms| *ms > 0).ok_or_else(|| {
            RpcError::invalid_params(format!(
                "{TOOL}: `timeoutMs` must be a positive integer of milliseconds"
            ))
        })?,
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `timeoutMs` must be a positive integer of milliseconds"
            )))
        }
    };
    let gate = asg.gate.clone();
    let result = append_and_reload(
        ledger,
        TOOL,
        json!({"type": "gate_started", "assignment": id, "gate": gate}),
    )?;
    // The Gate itself runs in a detached background process against the
    // candidate; its completion (green | red | uncertain) lands in the ledger
    // as `gate_completed`, observable via assignment_status / runtime_findings.
    gate::spawn_detached(state_dir, project_root, &id, timeout_ms)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    Ok(json!({
        "assignment": id,
        "state": result["state"],
        "gate": result["gate"],
        "timeoutMs": timeout_ms,
        "status": "running",
    }))
}

/// Supervisor `runtime_findings`: the verified findings of the runtime replayed
/// from the ledger — gate completion verdicts (with masked tail paths), handbacks
/// whose changed paths exceeded the declared Write Scope, and the currently due
/// Attention Letters (surfaced read-only, without consuming the delivery batch
/// that `room_status` records).
fn runtime_findings(ledger: &Ledger) -> Result<Value, RpcError> {
    let events = ledger.read_events();
    let mut findings = Vec::new();
    let letters = attention::deliver_letters(
        ledger,
        ledger::now_secs(),
        &attention::AttentionConfig::from_env(),
        false,
    )
    .map_err(|e| RpcError::internal(format!("runtime_findings: {e}")))?;
    for event in &events {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "gate_completed" => {
                let mut finding = json!({
                    "kind": "gate",
                    "assignment": event.get("assignment").cloned().unwrap_or(Value::Null),
                    "verdict": event.get("verdict").cloned().unwrap_or(Value::Null),
                    "seq": event.get("seq").cloned().unwrap_or(Value::Null),
                    "ts": event.get("ts").cloned().unwrap_or(Value::Null),
                });
                if let Some(tail) = event.get("tail") {
                    finding["tail"] = tail.clone();
                }
                if let Some(reason) = event.get("reason") {
                    finding["reason"] = reason.clone();
                }
                findings.push(finding);
            }
            "assignment_handed_back" => {
                if let Some(paths) = event.get("scopeViolations").and_then(Value::as_array)
                    && !paths.is_empty()
                {
                    findings.push(json!({
                        "kind": "scope.exceeded",
                        "assignment": event.get("assignment").cloned().unwrap_or(Value::Null),
                        "paths": paths,
                        "seq": event.get("seq").cloned().unwrap_or(Value::Null),
                        "ts": event.get("ts").cloned().unwrap_or(Value::Null),
                    }));
                }
            }
            _ => {}
        }
    }
    for letter in letters {
        let mut finding = json!({"kind": "attention"});
        if let (Some(base), Some(extra)) = (finding.as_object_mut(), letter.as_object()) {
            for (field, value) in extra {
                base.insert(field.clone(), value.clone());
            }
        }
        findings.push(finding);
    }
    Ok(json!({"findings": findings}))
}

/// Supervisor `room_status`: the live room snapshot of the Project — the
/// assignment board, the seats derivable from the ledger, and the current
/// batch of Attention Letters (whose delivery this call records; see
/// [`attention::deliver_letters`]).
fn room_status(ledger: &Ledger) -> Result<Value, RpcError> {
    const TOOL: &str = "room_status";
    let now = ledger::now_secs();
    let config = attention::AttentionConfig::from_env();
    let letters =
        attention::deliver_letters(ledger, now, &config, true)
            .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let state = RuntimeState::load(ledger);
    let assignments: Vec<Value> = state
        .assignments()
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "state": a.state,
                "disposition": a.disposition,
                "correlation": a.correlation,
                "gateVerdict": a.gate_verdict,
            })
        })
        .collect();
    // Peer seats are the dispatched correlations of live assignments; the Lead
    // seat is always the Project's single arbiter (replaced only through the
    // quiet-point sequence of `lead_replace_prepare`/`confirm`).
    let seats: Vec<Value> = state
        .assignments()
        .iter()
        .filter(|a| a.correlation.is_some() && !matches!(a.state.as_str(), "Stopped" | "Closed"))
        .map(|a| {
            json!({
                "role": "peer",
                "correlation": a.correlation,
                "assignment": a.id,
                "state": a.state,
            })
        })
        .chain(std::iter::once(json!({
            "role": "lead",
            "correlation": "lead",
            "attention": "active",
        })))
        .collect();

    Ok(json!({
        "project": ledger
            .project_dir()
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "generatedAt": now,
        "seats": seats,
        "assignments": assignments,
        "attention": {
            "allQuiet": letters.is_empty(),
            "letters": letters,
        },
    }))
}

/// Supervisor `attention_feedback { key, action: mute|snooze|downrank,
/// forSeconds? }`: rate a noisy Attention Letter by its stable key. The effect
/// (suppression, snooze window, downranking) is honored by every subsequent
/// batch (see [`attention::feedback_effect`]); the most recent feedback for a
/// key wins.
fn attention_feedback(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "attention_feedback";
    let key = arg_str(args, TOOL, "key")?;
    let action_raw = arg_str(args, TOOL, "action")?;
    let Some(action) = attention::FEEDBACK_ACTIONS
        .iter()
        .find(|action| **action == action_raw)
    else {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: invalid action `{action_raw}`: permitted actions are {}",
            attention::FEEDBACK_ACTIONS.join("|")
        )));
    };

    let mut event = json!({"type": "attention_feedback", "key": key, "action": action});
    if *action == "snooze" {
        match args.get("forSeconds") {
            None | Some(Value::Null) => {}
            Some(Value::Number(n)) => {
                let secs = n.as_u64().filter(|s| *s > 0).ok_or_else(|| {
                    RpcError::invalid_params(format!(
                        "{TOOL}: `forSeconds` must be a positive integer of seconds"
                    ))
                })?;
                event["forSeconds"] = json!(secs);
            }
            Some(_) => {
                return Err(RpcError::invalid_params(format!(
                    "{TOOL}: `forSeconds` must be a positive integer of seconds"
                )));
            }
        }
    }
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let config = attention::AttentionConfig::from_env();
    let effect = attention::feedback_effect(&ledger.read_events(), &key, ledger::now_secs(), &config);
    let effect_json = match effect {
        attention::FeedbackEffect::Muted => json!("muted"),
        attention::FeedbackEffect::Snoozed { until } => json!({"snoozedUntil": until}),
        attention::FeedbackEffect::Downranked => json!("downranked"),
        attention::FeedbackEffect::None => json!("none"),
    };
    Ok(json!({"key": key, "action": action, "effect": effect_json}))
}

/// Supervisor `message_lead { message }`: route a Human directive to the
/// Project's Lead. The durable guarantee is the `message_to_lead` ledger
/// mailbox event (observable from any session replaying the ledger); when the
/// Project's live Lead terminal is discoverable through `hc`, the message is
/// additionally delivered with `hc send` (best-effort, honestly reported).
fn message_lead(
    ledger: &Ledger,
    state_dir: &Path,
    project_root: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "message_lead";
    let message = arg_str(args, TOOL, "message")?;

    ledger
        .append(json!({"type": "message_to_lead", "from": "supervisor", "message": message}))
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let slug = project_slug(project_root);
    let hc_send = match crate::seats::project_lead_terminal(state_dir, &slug) {
        Ok(Some(address)) => match crate::hc::send_terminal_prompt(&address, &message) {
            Ok(()) => json!({"delivered": true, "target": address}),
            Err(reason) => json!({"delivered": false, "target": address, "reason": reason}),
        },
        Ok(None) => json!({
            "delivered": false,
            "reason": "no live lead terminal for the project; the mailbox event stays durable",
        }),
        Err(err) => json!({"delivered": false, "reason": err.to_string()}),
    };
    Ok(json!({"recorded": true, "hcSend": hc_send}))
}

/// Supervisor `lead_replace_prepare`: request a structured handoff from the
/// Lead at a verified quiet point. Refuses (JSON-RPC error) while any
/// assignment holds live work — a dispatched or reworking writable assignment
/// or an unanswered ask — because replacing the Lead mid-flight would strand
/// it. At a quiet point the room's handoff draft (assignment board, open
/// attention, rendered `handoffText`) is recorded as `lead_replace_prepared`
/// for review; exactly one replacement may be in flight at a time.
fn lead_replace_prepare(ledger: &Ledger) -> Result<Value, RpcError> {
    const TOOL: &str = "lead_replace_prepare";
    let events = ledger.read_events();
    if let Some(in_flight) = replacement::in_flight_replacement(&events) {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: a lead replacement (`{in_flight}`) is already prepared; confirm or cancel it before preparing another"
        )));
    }

    let state = RuntimeState::from_events(&events);
    let blockers = replacement::quiet_point_blockers(&state);
    if !blockers.is_empty() {
        let listed = blockers
            .iter()
            .map(|(id, reason)| format!("`{id}` ({reason})"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: not a quiet point — live work in flight: {listed}; close, answer, or take the handbacks before replacing the Lead"
        )));
    }

    let now = ledger::now_secs();
    let letters = attention::deliver_letters(
        ledger,
        now,
        &attention::AttentionConfig::from_env(),
        false,
    )
    .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    let slug = ledger
        .project_dir()
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let draft = replacement::build_draft(&slug, &state, &letters, now);
    let replacement_id = replacement::next_replacement_id(&events);
    let handoff_text = draft
        .get("handoffText")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    ledger
        .append(json!({
            "type": "lead_replace_prepared",
            "replacement": replacement_id,
            "draft": draft,
            "handoffText": handoff_text,
        }))
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    Ok(json!({
        "replacement": replacement_id,
        "quietPoint": {"verified": true},
        "assignments": draft["assignments"].clone(),
        "handoffText": handoff_text,
    }))
}

/// Supervisor `lead_replace_confirm { replacement?, handoffText?, agent? }`:
/// archive the old Lead and launch the successor Lead via the ticket-06 seat
/// launcher (same canonical project labels), seeded with the reviewed —
/// possibly edited — handoff text. The text lands in the successor's seed
/// record on disk (`lead-replacements/<id>-seed.md`, referenced from the
/// ledger event) and, when the successor terminal is addressable, in an
/// `hc send` of the seed. Requires the quiet point to still hold.
fn lead_replace_confirm(
    ledger: &Ledger,
    state_dir: &Path,
    project_root: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "lead_replace_confirm";
    let events = ledger.read_events();
    let Some(in_flight) = replacement::in_flight_replacement(&events) else {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: no prepared lead replacement to confirm; call `lead_replace_prepare` at a quiet point first"
        )));
    };
    if let Some(requested) = args.get("replacement").and_then(Value::as_str)
        && requested != in_flight
    {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: replacement `{requested}` is not the in-flight replacement (`{in_flight}`)"
        )));
    }

    // The quiet point must still hold at confirm time.
    let state = RuntimeState::from_events(&events);
    let blockers = replacement::quiet_point_blockers(&state);
    if !blockers.is_empty() {
        let listed = blockers
            .iter()
            .map(|(id, reason)| format!("`{id}` ({reason})"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: not a quiet point anymore — live work in flight: {listed}"
        )));
    }

    // The reviewed text: the edited argument, else the prepared draft verbatim.
    let handoff_text = match args.get("handoffText") {
        None | Some(Value::Null) => events
            .iter()
            .filter(|e| {
                e.get("type").and_then(Value::as_str) == Some("lead_replace_prepared")
                    && e.get("replacement").and_then(Value::as_str) == Some(in_flight.as_str())
            })
            .next_back()
            .and_then(|e| e.get("handoffText").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_default(),
        Some(Value::String(text)) => text.clone(),
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `handoffText` must be a string (the reviewed handoff text)"
            )));
        }
    };

    // The successor's agent family (the verified families of the launcher).
    let agent = match args.get("agent") {
        None | Some(Value::Null) => "codex".to_string(),
        Some(Value::String(raw)) => {
            let requested = raw.trim();
            AGENT_FAMILIES
                .iter()
                .find(|family| family.eq_ignore_ascii_case(requested))
                .map(|family| family.to_string())
                .ok_or_else(|| {
                    RpcError::invalid_params(format!(
                        "{TOOL}: invalid agent family `{requested}`: the verified seat families are {}",
                        AGENT_FAMILIES.join("|")
                    ))
                })?
        }
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `agent` must be a seat family string ({})",
                AGENT_FAMILIES.join("|")
            )));
        }
    };

    // 1. Archive the old Lead first: the launcher's reuse check must never
    //    hand the archived terminal back as the successor.
    let slug = project_slug(project_root);
    let terminal = crate::seats::project_lead_terminal(state_dir, &slug)
        .ok()
        .flatten();
    let already_archived = events.iter().any(|e| {
        e.get("type").and_then(Value::as_str) == Some("lead_archived")
            && e.get("replacement").and_then(Value::as_str) == Some(in_flight.as_str())
    });
    if !already_archived {
        let mut archive = json!({"type": "lead_archived", "replacement": in_flight});
        if let Some(address) = &terminal {
            archive["terminal"] = json!(address);
        }
        ledger
            .append(archive)
            .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    }

    // 2. Launch the successor Lead via the ticket-06 launcher (same project
    //    labels, Role Home scoping, and hygiene flags as every seat launch).
    let launch = crate::seats::launch_seat(state_dir, "lead", &agent, project_root, None);
    if let Err(err) = launch {
        return Err(RpcError::internal(format!(
            "{TOOL}: successor lead launch failed: {err}; the old lead is archived in the ledger — \
             re-run confirm to retry the launch, or investigate the launcher"
        )));
    }

    // 3. Seed the successor with the reviewed handoff: the durable seed record
    //    plus a best-effort `hc send` to the project's live lead terminal.
    let seed_record = replacement::seed_record_path(ledger, &in_flight);
    if let Some(parent) = seed_record.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            RpcError::internal(format!("{TOOL}: failed to create the seed record dir: {e}"))
        })?;
    }
    std::fs::write(&seed_record, &handoff_text).map_err(|e| {
        RpcError::internal(format!("{TOOL}: failed to write the seed record: {e}"))
    })?;
    let mut hc_seed = json!({"delivered": false, "reason": "no addressable lead terminal"});
    if let Ok(Some(address)) = crate::seats::project_lead_terminal(state_dir, &slug) {
        hc_seed = match crate::hc::send_terminal_prompt(&address, &handoff_text) {
            Ok(()) => json!({"delivered": true, "target": address}),
            Err(reason) => json!({"delivered": false, "target": address, "reason": reason}),
        };
    }

    let correlation = crate::seats::launch_correlation(&agent, "lead", &None);
    ledger
        .append(json!({
            "type": "lead_replace_confirmed",
            "replacement": in_flight,
            "handoffText": handoff_text,
            "seedRecord": seed_record.to_string_lossy(),
            "successorAgent": agent,
            "successorCorrelation": correlation,
        }))
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    Ok(json!({
        "replacement": in_flight,
        "state": "confirmed",
        "archived": true,
        "terminal": terminal,
        "successor": {"agent": agent, "correlation": correlation},
        "seedRecord": seed_record.to_string_lossy(),
        "handoffText": handoff_text,
        "hcSeed": hc_seed,
    }))
}

/// Supervisor `lead_replace_cancel { replacement? }`: abort an in-flight Lead
/// replacement. Nothing is archived and nothing is launched — the old Lead
/// stays the Project's active arbiter.
fn lead_replace_cancel(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "lead_replace_cancel";
    let events = ledger.read_events();
    let Some(in_flight) = replacement::in_flight_replacement(&events) else {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: no in-flight lead replacement to cancel"
        )));
    };
    if let Some(requested) = args.get("replacement").and_then(Value::as_str)
        && requested != in_flight
    {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: replacement `{requested}` is not the in-flight replacement (`{in_flight}`)"
        )));
    }
    ledger
        .append(json!({"type": "lead_replace_cancelled", "replacement": in_flight}))
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    Ok(json!({
        "replacement": in_flight,
        "state": "cancelled",
        "lead": "active",
    }))
}

// ---------------------------------------------------------------------------
// Peer tools (server-derived identity; spec user stories 23-24)
// ---------------------------------------------------------------------------
/// Structured `Challenge Signal` stances a Peer may raise via `ask`
/// (spec: Role-Asymmetric MCP Surface, Peer paragraph).
pub const CHALLENGE_SIGNALS: &[&str] = &["REOPEN_REQUEST", "DEPENDENCY_REQUEST", "BLOCKED"];

/// Argument keys a Peer tool call must never carry: identity, assignment, and
/// workspace are resolved server-side from the session correlation, never from
/// tool arguments (spec user story 23).
const PEER_IDENTITY_ARG_KEYS: &[&str] = &[
    "assignment",
    "assignment_id",
    "seat",
    "workspace",
    "correlation",
    "project",
];

/// Refuses any peer tool call that tries to supply its own identity binding.
fn refuse_identity_args(tool: &str, args: &Value) -> Result<(), RpcError> {
    for key in PEER_IDENTITY_ARG_KEYS {
        if args.get(key).is_some_and(|v| !v.is_null()) {
            return Err(RpcError::invalid_params(format!(
                "{tool}: `{key}` in tool args is refused: identity, assignment, and workspace are derived server-side from the session correlation"
            )));
        }
    }
    Ok(())
}

/// Resolves the peer's binding server-side: the most recently dispatched
/// assignment registered with this correlation (the binding is written by
/// `assignment_dispatch`), together with its workspace.
fn resolve_peer_binding(
    ledger: &Ledger,
    tool: &str,
    correlation: Option<&str>,
) -> Result<Assignment, RpcError> {
    let Some(correlation) = correlation.map(str::trim).filter(|c| !c.is_empty()) else {
        return Err(RpcError::invalid_params(format!(
            "{tool}: peer identity requires a session correlation (--correlation / HOTPOT_CORRELATION, injected at seat launch); the server never derives identity from tool arguments"
        )));
    };
    let state = RuntimeState::load(ledger);
    // The latest `assignment_dispatched` event carrying this correlation wins:
    // the ledger, not the caller, decides which assignment the seat works on.
    let bound = ledger
        .read_events()
        .iter()
        .rev()
        .find(|e| {
            e.get("type").and_then(Value::as_str) == Some("assignment_dispatched")
                && e.get("correlation").and_then(Value::as_str) == Some(correlation)
        })
        .and_then(|e| e.get("assignment").and_then(Value::as_str))
        .and_then(|id| state.assignment(id));
    let assignment = bound.ok_or_else(|| {
        RpcError::invalid_params(format!(
            "{tool}: correlation `{correlation}` is not bound to any dispatched assignment in this project"
        ))
    })?;
    if assignment
        .workspace
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return Err(RpcError::internal(format!(
            "{tool}: the binding for correlation `{correlation}` records no workspace; re-dispatch the assignment"
        )));
    }
    Ok(assignment.clone())
}

/// Peer `ask`: raises a question (optionally a structured `Challenge Signal`)
/// for the Lead as a ledger event answerable via `assignment_answer`.
fn peer_ask(ledger: &Ledger, correlation: Option<&str>, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "ask";
    refuse_identity_args(TOOL, args)?;
    let question = arg_str(args, TOOL, "question")?;
    let signal = match args.get("signal") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let value = s.trim();
            if CHALLENGE_SIGNALS.contains(&value) {
                Some(value.to_string())
            } else {
                return Err(RpcError::invalid_params(format!(
                    "{TOOL}: invalid Challenge Signal `{value}`: permitted signals are {}",
                    CHALLENGE_SIGNALS.join("|")
                )));
            }
        }
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `signal` must be a Challenge Signal string ({})",
                CHALLENGE_SIGNALS.join("|")
            )))
        }
    };

    let asg = resolve_peer_binding(ledger, TOOL, correlation)?;
    require_state(TOOL, "asking", &asg, &["Dispatched", "Asking"])?;

    let mut event = json!({
        "type": "assignment_asked",
        "assignment": asg.id,
        "correlation": asg.correlation,
        "question": question,
    });
    if let Some(signal) = &signal {
        event["signal"] = json!(signal);
    }
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let reloaded = RuntimeState::load(ledger)
        .assignment(&asg.id)
        .expect("assignment exists before its lifecycle event")
        .clone();
    let mut result = json!({
        "state": reloaded.state,
        "question": question,
        "routedTo": "lead",
    });
    if let Some(signal) = &signal {
        result["signal"] = json!(signal);
    }
    if let Some(answer) = &reloaded.last_answer {
        result["lastAnswer"] = json!(answer);
    }
    Ok(result)
}

/// Peer `handoff`: submits finished work, validated against live Git evidence.
///
/// Writable assignments are validated in order (spec user story 24): a clean
/// working tree, a `Candidate` derived from `baseCommit..HEAD` with at least one
/// commit, the required Gate run after the peer's last write, and finally a
/// `scope.exceeded` flag (not a refusal) for changed paths outside the declared
/// Write Scope. Read-only assignments only require a clean tree and make no
/// candidate or lease claims.
fn peer_handoff(
    ledger: &Ledger,
    correlation: Option<&str>,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "handoff";
    refuse_identity_args(TOOL, args)?;
    let summary = arg_str(args, TOOL, "summary")?;
    let asg = resolve_peer_binding(ledger, TOOL, correlation)?;
    require_state(
        TOOL,
        "handing back",
        &asg,
        &["Dispatched", "Asking", "ReworkRequested"],
    )?;
    let workspace = Path::new(asg.workspace.as_deref().unwrap_or_default());

    // (1) Clean working tree in the peer's workspace.
    let dirty = leases::porcelain_entries(workspace)?;
    if !dirty.is_empty() {
        let sample: Vec<String> = dirty.iter().take(3).cloned().collect();
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: git precondition failed: the working tree of {} is not clean (`git status --porcelain=v1 --untracked-files=all` reports {} entr{}): {}; commit or revert everything before handing back",
            workspace.display(),
            dirty.len(),
            if dirty.len() == 1 { "y" } else { "ies" },
            sample.join(", ")
        )));
    }

    // Read-only assignments (no Write Scope) claim neither candidate nor lease.
    if asg.write_scope.is_empty() {
        ledger
            .append(json!({
                "type": "assignment_handed_back",
                "assignment": asg.id,
                "correlation": asg.correlation,
                "summary": summary,
            }))
            .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
        let reloaded = RuntimeState::load(ledger)
            .assignment(&asg.id)
            .expect("assignment exists before its lifecycle event")
            .clone();
        return Ok(json!({
            "state": reloaded.state,
            "assignment": asg.id,
            "candidate": Value::Null,
            "readOnly": true,
            "handedBackAt": reloaded.handed_back_at,
        }));
    }

    // (2) Candidate: at least one commit in baseCommit..HEAD.
    let candidate = leases::head_commit(workspace)?;
    let range = format!("{}..{}", asg.base_commit, candidate);
    let count = workspace_git(workspace, &["rev-list", "--count", &range])?
        .trim()
        .parse::<u64>()
        .unwrap_or(0);
    if count == 0 {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: no Candidate: the workspace HEAD `{candidate}` has no commits past the assignment baseCommit `{}`; baseCommit..HEAD must contain at least one commit before handing back",
            asg.base_commit
        )));
    }

    // (3) The required Gate ran after the peer's last write (ledger evidence).
    let events = ledger.read_events();
    let last_write = evidence::latest_evidence_seq(&events, &asg.id, evidence::WRITE_EVENT);
    let last_gate = evidence::latest_evidence_seq(&events, &asg.id, evidence::GATE_EVENT);
    match (last_gate, last_write) {
        (None, _) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: gate precondition failed: no gate-run record for `{}`; run the assignment Gate (`{}`) in the workspace after the last write, then hand off again",
                asg.id, asg.gate
            )));
        }
        (Some(gate), Some(write)) if gate < write => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: gate precondition failed: the Gate last ran before the peer's last write (gate seq {gate} < write seq {write}); re-run the Gate (`{}`) after the final write, then hand off again",
                asg.gate
            )));
        }
        _ => {}
    }

    // (4) Changed paths outside the declared Write Scope are flagged, not refused.
    let changed = workspace_git(workspace, &["diff", "--name-only", &range])?;
    let violations: Vec<String> = changed
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| !asg.write_scope.iter().any(|glob| leases::glob_matches(glob, p)))
        .map(str::to_string)
        .collect();

    let mut event = json!({
        "type": "assignment_handed_back",
        "assignment": asg.id,
        "correlation": asg.correlation,
        "summary": summary,
        "candidate": candidate,
        "baseCommit": asg.base_commit,
    });
    if !violations.is_empty() {
        event["scopeViolations"] = json!(violations);
    }
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let reloaded = RuntimeState::load(ledger)
        .assignment(&asg.id)
        .expect("assignment exists before its lifecycle event")
        .clone();
    Ok(json!({
        "state": reloaded.state,
        "assignment": asg.id,
        "candidate": candidate,
        "baseCommit": asg.base_commit,
        "range": range,
        "scope": {"exceeded": !violations.is_empty(), "paths": violations},
        "handedBackAt": reloaded.handed_back_at,
    }))
}

/// Runs `git -C <workspace> <args..>` and returns trimmed stdout on success.
fn workspace_git(workspace: &Path, args: &[&str]) -> Result<String, RpcError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .map_err(|e| RpcError::internal(format!("failed to invoke git: {e}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(RpcError::internal(format!(
            "git {:?} failed in {}: {}",
            args,
            workspace.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

//! Seat launching & the 1:1 Project topology on Herdr (ticket 06).
//!
//! `hotpot seat launch` creates an `hc` terminal running the vendor CLI scoped to one
//! seat's Role Home, attaching the canonical label set that binds every terminal strictly
//! 1:1 to one Project (spec: "Implementation Decisions → 1:1 Project Topology on Herdr"):
//!
//! ```text
//! hotpot.v1=true
//! hotpot.project=<project-slug>       (<basename>-<sha1(gitRoot)[0..6]>)
//! hotpot.role=<supervisor|lead|peer>
//! hotpot.agent=<codex|claude|pi>
//! hotpot.assignment=<asg_id>          (Peers only)
//! hotpot.parent=<lead_seat_id>        (Peers only)
//! hotpot.correlation=<corr_id>
//! ```
//!
//! The Project slug reuses the shared `runtime::project_slug` helper so `hc` labels and the
//! runtime event log canonicalize a Project identically (ticket 07 reuses it too). Before
//! launching a Lead, existing `hc` terminals are discovered by labels: a live or
//! idle/resumable Lead for the Project is reused instead of duplicated, and two live Leads
//! halt with an escalation instead of spawning a third control plane.

use crate::hc::{self, HcTerminalSummary};
use crate::hygiene;
use crate::provisioner::{PlanError, find_on_path};
use crate::role_homes::{role_home_dir, seat_correlation};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The canonical version marker carried by every hotpot-created `hc` terminal.
pub const LABEL_V1: &str = "hotpot.v1";

/// Terminal statuses under which a Lead is considered live or idle/resumable (a listing
/// without a status is read as idle, the same default `hotpot status` applies).
const REUSABLE_LEAD_STATUSES: &[&str] = &["live", "idle", "resumable"];

/// The outcome of a seat launch.
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// A new `hc` terminal was created for the seat.
    Launched,
    /// An existing Lead terminal for the Project was reused; nothing was created.
    Reused { address: String },
}

/// The peer-only launch inputs (`--assignment` / `--parent`): required for `peer`, rejected
/// for `supervisor`/`lead` because the canonical label set carries them on Peers only.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PeerBinding {
    pub assignment: String,
    pub parent: String,
}

/// The per-seat MCP correlation id stamped as `hotpot.correlation`: Supervisor/Lead reuse the
/// deterministic per-seat correlation pinned in their Role Home MCP wiring; a Peer is
/// assignment-scoped so each Peer terminal binds to exactly one Assignment.
pub fn launch_correlation(agent: &str, role: &str, binding: &Option<PeerBinding>) -> String {
    match binding {
        Some(binding) => format!("{agent}-peer-{}", binding.assignment),
        None => seat_correlation(agent, role),
    }
}

/// Validates the seat name pair, returning a stable diagnostic naming the invalid value.
fn validate_seat(agent: &str, role: &str) -> Result<(), PlanError> {
    if !["codex", "claude", "pi"].contains(&agent) {
        return Err(PlanError::new(
            "invalid-agent",
            format!("invalid agent `{agent}` (expected codex|claude|pi)"),
        ));
    }
    if !["supervisor", "lead", "peer"].contains(&role) {
        return Err(PlanError::new(
            "invalid-role",
            format!("invalid role `{role}` (expected supervisor|lead|peer)"),
        ));
    }
    Ok(())
}

/// Environment variables scoping the vendor process to `role_home` (`--env` flags on the
/// `hc run` argv): `$HOME` always, plus the vendor's canonical home override — the same
/// scoping `hotpot auth login` applies, so no path can reach an operator home.
fn vendor_home_env(agent: &str) -> &'static [&'static str] {
    match agent {
        "codex" => &["HOME", "CODEX_HOME"],
        "claude" => &["HOME", "CLAUDE_CONFIG_DIR"],
        _ => &["HOME"],
    }
}

/// The vendor CLI's launch arguments pinning the Capability Hygiene closures of ticket 03
/// that ride on the command line (Codex's closures live in its `config.json`, not flags).
fn vendor_hygiene_args(agent: &str) -> Vec<String> {
    match agent {
        "claude" => hygiene::claude_deny_launch_args(),
        "pi" => hygiene::PI_LAUNCH_ARGS.iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// True when the terminal is a hotpot-created terminal of the given role for the Project.
fn is_seat_terminal(terminal: &HcTerminalSummary, slug: &str, role: &str) -> bool {
    terminal.label(LABEL_V1) == Some("true")
        && terminal.label("hotpot.role") == Some(role)
        && terminal.label("hotpot.project") == Some(slug)
}

/// True when the terminal's status marks it live or idle/resumable (missing → idle).
fn reusable(terminal: &HcTerminalSummary) -> bool {
    match terminal.status.as_deref() {
        None => true,
        Some(status) => REUSABLE_LEAD_STATUSES.contains(&status),
    }
}

/// Terminal addresses of Leads archived by a completed quiet-point replacement
/// (the `lead_archived { terminal }` runtime ledger events, ticket 11). An
/// archived Lead's terminal is never a reuse candidate: the successor, not the
/// archived terminal, is the Project's Lead.
pub fn archived_lead_addresses(home: &Path, slug: &str) -> Vec<String> {
    let ledger = crate::runtime::Ledger::at(
        home.join("runtime")
            .join("v1")
            .join(slug)
            .join("ledger.jsonl"),
    );
    ledger
        .read_events()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("lead_archived"))
        .filter_map(|e| e.get("terminal").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Existing live or idle/resumable Lead terminals for the Project (discovered
/// by labels), excluding terminals archived by a completed Lead replacement.
fn reusable_leads(home: &Path, slug: &str) -> Result<Vec<HcTerminalSummary>, PlanError> {
    let archived = archived_lead_addresses(home, slug);
    Ok(hc::list_terminals_strict()?
        .into_iter()
        .filter(|t| is_seat_terminal(t, slug, "lead") && reusable(t))
        .filter(|t| !archived.contains(&t.address))
        .collect())
}

/// The address of the Project's live (or idle/resumable) Lead terminal — the
/// terminal `message_lead` addresses and a Lead replacement archives. `None`
/// when no live Lead terminal carries the Project's labels.
pub fn project_lead_terminal(home: &Path, slug: &str) -> Result<Option<String>, PlanError> {
    let archived = archived_lead_addresses(home, slug);
    Ok(hc::list_terminals_strict()?
        .into_iter()
        .find(|t| is_seat_terminal(t, slug, "lead") && reusable(t) && !archived.contains(&t.address))
        .map(|t| t.address))
}

/// Assembles the full `hc run` argv (after the `run` word) for one seat terminal: persistent
/// terminal flags, the terminal cwd, every canonical label, the Role Home `--env` scoping,
/// and the vendor command with its hygiene flags after `--`.
fn seat_argv(
    role_home: &Path,
    project_root: &Path,
    slug: &str,
    role: &str,
    agent: &str,
    binding: &Option<PeerBinding>,
    vendor_bin: &Path,
) -> Vec<String> {
    let mut argv: Vec<String> = vec!["--bg".into(), "--keep".into()];
    argv.push("--cwd".into());
    argv.push(project_root.to_string_lossy().into_owned());

    let mut label = |k: &str, v: &str| {
        argv.push("--label".into());
        argv.push(format!("{k}={v}"));
    };
    label(LABEL_V1, "true");
    label("hotpot.project", slug);
    label("hotpot.role", role);
    label("hotpot.agent", agent);
    if let Some(binding) = binding {
        label("hotpot.assignment", &binding.assignment);
        label("hotpot.parent", &binding.parent);
    }
    label(
        "hotpot.correlation",
        &launch_correlation(agent, role, binding),
    );

    for var in vendor_home_env(agent) {
        argv.push("--env".into());
        argv.push(format!("{var}={}", role_home.to_string_lossy()));
    }

    argv.push("--".into());
    argv.push(vendor_bin.to_string_lossy().into_owned());
    argv.extend(vendor_hygiene_args(agent));
    argv
}

/// The best-effort delivery report of a `--prompt` directive (ticket nui/04).
#[derive(Debug)]
pub struct PromptDispatch {
    /// The terminal the directive was addressed to, when one was resolved.
    pub target: Option<String>,
    /// Whether `hc send` accepted the directive.
    pub delivered: bool,
    /// Honest failure reason when `delivered` is false.
    pub reason: Option<String>,
}

/// Delivers the `--prompt` directive of `hotpot seat launch` after a launch or
/// reuse (ticket nui/04): the directive is recorded first as a durable
/// `message_to_lead` mailbox event (from the Human, `to` the seat — the
/// projection `hotpot status` serves), then sent into the seat's terminal via
/// `hc send`, best-effort and honestly reported. A reused Lead is addressed by
/// its terminal address; a fresh terminal is resolved from the `hc` listing by
/// the launch's canonical correlation labels.
pub fn dispatch_prompt(
    home: &Path,
    slug: &str,
    role: &str,
    agent: &str,
    binding: &Option<PeerBinding>,
    outcome: &LaunchOutcome,
    prompt: &str,
) -> Result<PromptDispatch, PlanError> {
    let correlation = launch_correlation(agent, role, binding);
    let ledger = crate::runtime::Ledger::at(
        home.join("runtime")
            .join("v1")
            .join(slug)
            .join("ledger.jsonl"),
    );
    ledger.ensure_parent_dir().map_err(|e| {
        PlanError::new("io", format!("failed to record the directive: {e}"))
    })?;
    // A peer directive addresses the assignment-scoped correlation (the seat's
    // identity); the single-seat roles are addressed by role name.
    let to = match role {
        "peer" => correlation.as_str(),
        other => other,
    };
    ledger
        .append(json!({
            "type": "message_to_lead",
            "from": "human",
            "to": to,
            "message": prompt,
            "seat": correlation,
        }))
        .map_err(|e| PlanError::new("io", format!("failed to record the directive: {e}")))?;

    let target = match outcome {
        LaunchOutcome::Reused { address } => Some(address.clone()),
        LaunchOutcome::Launched => hc::list_terminals()
            .into_iter()
            .find(|t| {
                t.label(LABEL_V1) == Some("true")
                    && t.label("hotpot.project") == Some(slug)
                    && t.label("hotpot.correlation") == Some(correlation.as_str())
            })
            .map(|t| t.address),
    };
    let Some(target) = target else {
        return Ok(PromptDispatch {
            target: None,
            delivered: false,
            reason: Some(
                "no hc terminal carries this launch's correlation labels".to_string(),
            ),
        });
    };
    match hc::send_terminal_prompt(&target, prompt) {
        Ok(()) => Ok(PromptDispatch {
            target: Some(target),
            delivered: true,
            reason: None,
        }),
        Err(reason) => Ok(PromptDispatch {
            target: Some(target),
            delivered: false,
            reason: Some(reason),
        }),
    }
}

/// Launches (or reuses) one seat's `hc` terminal for the Project at `project_root`:
///
/// - every launch validates the role/agent pair and the peer-only binding,
/// - a Lead launch first discovers existing live or idle/resumable Leads for the Project:
///   exactly one is reused, two or more halt with an escalation naming the duplicates,
/// - a fresh terminal is created via the injectable `hc` runner with the full canonical
///   label set, the Role Home `--env` scoping, and the vendor hygiene flags.
pub fn launch_seat(
    home: &Path,
    role: &str,
    agent: &str,
    project_root: &Path,
    binding: Option<PeerBinding>,
) -> Result<LaunchOutcome, PlanError> {
    validate_seat(agent, role)?;
    match (role, &binding) {
        ("peer", None) => {
            return Err(PlanError::new(
                "peer-binding-missing",
                "peer launches require --assignment <id> and --parent <lead-terminal> \
                 (the canonical label set carries hotpot.assignment and hotpot.parent on \
                 Peers only)",
            ));
        }
        ("peer", Some(binding)) if binding.assignment.is_empty() || binding.parent.is_empty() => {
            return Err(PlanError::new(
                "peer-binding-missing",
                "peer launches require non-empty --assignment <id> and --parent <lead-terminal>",
            ));
        }
        ("peer", _) => {}
        (_, Some(_)) => {
            return Err(PlanError::new(
                "peer-binding-unexpected",
                format!(
                    "--assignment/--parent apply to peer launches only; the canonical label \
                     set carries hotpot.assignment and hotpot.parent on Peers, not the {role} seat"
                ),
            ));
        }
        _ => {}
    }

    let project_root: PathBuf = project_root
        .canonicalize()
        .unwrap_or_else(|_| crate::provisioner::normalize(project_root));
    let slug = crate::runtime::project_slug(&project_root);

    // 1:1 topology guard (Lead only, per ticket 06): reuse before duplicating, halt on
    // duplicates so each Project keeps a single unambiguous technical arbiter.
    if role == "lead" {
        let leads = reusable_leads(home, &slug)?;
        if let [only] = leads.as_slice() {
            return Ok(LaunchOutcome::Reused {
                address: only.address.clone(),
            });
        }
        if leads.len() >= 2 {
            let duplicates: Vec<String> =
                leads.iter().map(|t| t.address.clone()).collect();
            return Err(PlanError::new(
                "duplicate-leads",
                format!(
                    "two or more live lead terminals already exist for project {slug} \
                     ({}); refusing to launch another lead — resolve the duplicates \
                     before continuing",
                    duplicates.join(", ")
                ),
            ));
        }
    }

    let role_home = role_home_dir(home, agent, role);
    if !role_home.is_dir() {
        return Err(PlanError::new(
            "role-home-missing",
            format!(
                "Role Home {} does not exist; run `hotpot setup --apply` first",
                role_home.display()
            ),
        ));
    }

    // On-demand credential seeding (the same rules `hotpot setup` applies): a seat whose
    // credential is still missing inherits an independent copy of the operator's
    // file-backed credential before the vendor terminal starts, so it authenticates
    // immediately. The operator's HOME is a read-only input.
    let operator_home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    crate::credential_seeding::seed_seat_credential(home, &operator_home, agent, role)?;

    let vendor_bin = find_on_path(agent).ok_or_else(|| {
        PlanError::new(
            "vendor-not-found",
            format!("no `{agent}` binary on PATH; install the {agent} CLI to launch the seat"),
        )
    })?;

    let argv = seat_argv(
        &role_home,
        &project_root,
        &slug,
        role,
        agent,
        &binding,
        &vendor_bin,
    );
    hc::run_terminal(&argv)?;

    // The launch is durable in the project ledger so `hotpot status` can project
    // the seat even while `hc` is unreachable (a reused Lead already carries its
    // earlier launch record). Best-effort fields only: the terminal address and
    // status always come from the live `hc` listing.
    let mut event = json!({
        "type": "seat_launched",
        "seat": launch_correlation(agent, role, &binding),
        "role": role,
        "agent": agent,
    });
    if let Some(binding) = &binding {
        event["assignment"] = json!(binding.assignment);
    }
    let ledger = crate::runtime::Ledger::at(
        home.join("runtime")
            .join("v1")
            .join(&slug)
            .join("ledger.jsonl"),
    );
    ledger.ensure_parent_dir().map_err(|e| {
        PlanError::new("io", format!("failed to record the seat launch: {e}"))
    })?;
    ledger
        .append(event)
        .map_err(|e| PlanError::new("io", format!("failed to record the seat launch: {e}")))?;
    Ok(LaunchOutcome::Launched)
}

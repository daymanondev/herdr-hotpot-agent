//! Desired-state provisioner engine over Hotpot Home.
//!
//! The provisioner layer is a stateless desired-state generator: `hotpot setup` builds a
//! [`Plan`] (planned directories, files, symlinks, and `hc` profile changes), prints it as a
//! dry-run by default, and only materializes it with `--apply`. All writes are strictly
//! confined to Hotpot Home (`HOTPOT_HOME`, defaulting to `~/.hotpot`); operator homes such as
//! `~/.codex`, `~/.claude`, and `~/.pi/agent` are read-only inputs, and `runtime/v1/` (the
//! runtime coordinator's append-only state) is preserve-only — never planned, never written.

use crate::contracts::RenderedContracts;
use crate::hygiene;
use crate::role_homes;
use serde_json::json;
use std::env;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Agent families provisioned by the base Hotpot Home plan.
pub const AGENTS: &[&str] = &["codex", "claude", "pi"];

/// Roles each agent family is provisioned for.
pub const ROLES: &[&str] = &["supervisor", "lead", "peer"];

/// Relative location of the runtime coordinator's append-only state under Hotpot Home.
pub const RUNTIME_STATE_DIR: &str = "runtime/v1";

/// Credential file names owned by individual Role Homes (scanned for `hotpot remove` warnings).
pub const CREDENTIAL_FILE_NAMES: &[&str] = &["auth.json", ".credentials.json", "credentials.json"];

/// A single failure of the provisioner, with a stable diagnostic code for stderr.
#[derive(Debug)]
pub struct PlanError {
    pub code: &'static str,
    pub message: String,
}

impl PlanError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// One desired-state change inside (or, for `hc`, about) Hotpot Home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanItem {
    /// A directory that must exist.
    Dir(PathBuf),
    /// A file with fully rendered contents, written atomically.
    File { path: PathBuf, contents: String },
    /// A symlink that must point at `target`.
    Symlink { path: PathBuf, target: PathBuf },
    /// An `hc profile set <profile> <key> <value>` change applied via the `hc` CLI.
    HcProfileSet {
        profile: String,
        key: String,
        value: String,
    },
}

impl PlanItem {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Dir(path) | Self::File { path, .. } | Self::Symlink { path, .. } => Some(path),
            Self::HcProfileSet { .. } => None,
        }
    }
}

/// A validated desired-state plan over a Hotpot Home.
#[derive(Debug, Clone)]
pub struct Plan {
    pub home: PathBuf,
    pub items: Vec<PlanItem>,
    /// Non-item report lines the plan must surface in both dry-run and apply output (e.g.
    /// deterministic skill-collision exclusions).
    pub notes: Vec<String>,
}

impl Plan {
    /// Rejects any plan whose filesystem writes would escape `home` or touch the
    /// preserve-only `runtime/` state tree.
    pub fn validate(&self) -> Result<(), PlanError> {
        let home = normalize(&self.home);
        for item in &self.items {
            let Some(path) = item.path() else {
                continue;
            };
            let path = normalize(path);
            if path == home || !path.starts_with(&home) {
                return Err(PlanError::new(
                    "confinement-escape",
                    format!(
                        "planned write {} escapes Hotpot Home {}",
                        path.display(),
                        home.display()
                    ),
                ));
            }
            if path.starts_with(home.join("runtime")) {
                return Err(PlanError::new(
                    "runtime-preserve-only",
                    format!(
                        "planned write {} targets preserve-only runtime state under {}",
                        path.display(),
                        home.join(RUNTIME_STATE_DIR).display()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Renders the `plan: ...` lines printed by a dry-run (one line per planned change).
    pub fn render_plan_lines(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|item| match item {
                PlanItem::Dir(path) => format!("plan: dir {}", path.display()),
                PlanItem::File { path, .. } => format!("plan: file {}", path.display()),
                PlanItem::Symlink { path, target } => {
                    format!("plan: link {} -> {}", path.display(), target.display())
                }
                PlanItem::HcProfileSet {
                    profile,
                    key,
                    value,
                } => format!("plan: hc profile set {profile} {key} {value}"),
            })
            .collect()
    }

    /// Prints the dry-run report without mutating anything: the plan's report notes first
    /// (e.g. collision exclusions), then every planned change.
    pub fn print_dry_run(&self, out: &mut impl Write) -> std::io::Result<()> {
        for note in &self.notes {
            writeln!(out, "{note}")?;
        }
        for line in self.render_plan_lines() {
            writeln!(out, "{line}")?;
        }
        writeln!(
            out,
            "dry-run: no changes applied (pass --apply to provision {})",
            self.home.display()
        )
    }

    /// Materializes exactly the planned set under `home`.
    ///
    /// Pre-flights every planned path shape before touching disk (no partial state), writes
    /// files and symlinks atomically via a temporary sibling + rename, and applies `hc`
    /// profile changes through the discovered `hc` binary. Report notes (collision
    /// exclusions) are printed before the applied changes.
    pub fn apply(&self, out: &mut impl Write) -> Result<(), PlanError> {
        self.validate()?;
        self.preflight_path_shapes()?;

        for note in &self.notes {
            writeln!(out, "{note}")
                .map_err(|e| PlanError::new("io", format!("failed to print plan note: {e}")))?;
        }
        for item in &self.items {
            match item {
                PlanItem::Dir(path) => {
                    fs::create_dir_all(path)
                        .map_err(|e| PlanError::new("io", format!("failed to create dir {}: {e}", path.display())))?;
                    writeln!(out, "applied: dir {}", path.display())
                        .map_err(|e| PlanError::new("io", format!("failed to print plan progress: {e}")))?;
                }
                PlanItem::File { path, contents } => {
                    atomic_write_file(path, contents)?;
                    writeln!(out, "applied: file {}", path.display())
                        .map_err(|e| PlanError::new("io", format!("failed to print plan progress: {e}")))?;
                }
                PlanItem::Symlink { path, target } => {
                    atomic_symlink(path, target)?;
                    writeln!(out, "applied: link {} -> {}", path.display(), target.display())
                        .map_err(|e| PlanError::new("io", format!("failed to print plan progress: {e}")))?;
                }
                PlanItem::HcProfileSet {
                    profile,
                    key,
                    value,
                } => {
                    apply_hc_profile(profile, key, value)?;
                    writeln!(out, "applied: hc profile set {profile} {key} {value}")
                        .map_err(|e| PlanError::new("io", format!("failed to print plan progress: {e}")))?;
                }
            }
        }
        Ok(())
    }

    /// Refuses to proceed when a planned path is already occupied by an unexpected shape
    /// (e.g. a directory where a file is planned) — before any disk mutation happens.
    fn preflight_path_shapes(&self) -> Result<(), PlanError> {
        for item in &self.items {
            match item {
                PlanItem::Dir(path) => {
                    if let Ok(meta) = fs::symlink_metadata(path)
                        && !meta.is_dir()
                    {
                        return Err(PlanError::new(
                            "path-shape-conflict",
                            format!(
                                "planned directory {} is occupied by a non-directory; refusing to overwrite",
                                path.display()
                            ),
                        ));
                    }
                }
                PlanItem::File { path, .. } => {
                    if let Ok(meta) = fs::symlink_metadata(path)
                        && meta.is_dir()
                    {
                        return Err(PlanError::new(
                            "path-shape-conflict",
                            format!(
                                "planned file {} is occupied by a directory; refusing to overwrite",
                                path.display()
                            ),
                        ));
                    }
                }
                PlanItem::Symlink { path, .. } => {
                    if let Ok(meta) = fs::symlink_metadata(path)
                        && meta.is_dir()
                    {
                        return Err(PlanError::new(
                            "path-shape-conflict",
                            format!(
                                "planned symlink {} is occupied by a directory; refusing to overwrite",
                                path.display()
                            ),
                        ));
                    }
                }
                PlanItem::HcProfileSet { .. } => {}
            }
        }
        Ok(())
    }
}

/// Temporary sibling name used for atomic file/symlink writes (`<name>.hotpot-tmp.<pid>`).
fn temp_sibling(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{file_name}.hotpot-tmp.{}", std::process::id()))
}

/// Writes `contents` to `path` atomically: a temporary sibling is written first and then
/// renamed over the destination, so a `Seat` never observes a half-written file. Any
/// leftover temporary is removed on failure.
pub(crate) fn atomic_write_file(path: &Path, contents: &str) -> Result<(), PlanError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| PlanError::new("io", format!("failed to create parent of {}: {e}", path.display())))?;
    }
    let tmp = temp_sibling(path);
    if let Err(e) = fs::write(&tmp, contents) {
        let _ = fs::remove_file(&tmp);
        return Err(PlanError::new(
            "io",
            format!("failed to write temp sibling {}: {e}", tmp.display()),
        ));
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(PlanError::new(
            "io",
            format!("failed to rename {} over {}: {e}", tmp.display(), path.display()),
        ));
    }
    Ok(())
}

/// Points `path` at `target` atomically (temporary sibling symlink, then rename).
fn atomic_symlink(path: &Path, target: &Path) -> Result<(), PlanError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| PlanError::new("io", format!("failed to create parent of {}: {e}", path.display())))?;
    }
    let tmp = temp_sibling(path);
    if let Err(e) = std::os::unix::fs::symlink(target, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(PlanError::new(
            "io",
            format!("failed to create temp symlink {}: {e}", tmp.display()),
        ));
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(PlanError::new(
            "io",
            format!("failed to rename {} over {}: {e}", tmp.display(), path.display()),
        ));
    }
    Ok(())
}

/// Applies an `hc profile set` change through the `hc` binary discovered on `PATH`.
fn apply_hc_profile(profile: &str, key: &str, value: &str) -> Result<(), PlanError> {
    let hc = find_on_path("hc").ok_or_else(|| {
        PlanError::new(
            "hc-not-found",
            "hc is not on PATH; cannot apply the planned hc profile change",
        )
    })?;
    let status = Command::new(&hc)
        .args(["profile", "set", profile, key, value])
        .status()
        .map_err(|e| PlanError::new("io", format!("failed to invoke hc profile set: {e}")))?;
    if !status.success() {
        return Err(PlanError::new(
            "hc-profile-failed",
            format!("hc profile set {profile} {key} {value} exited with {status}"),
        ));
    }
    Ok(())
}

/// Lexically normalizes `path` to an absolute path without requiring it to exist.
pub fn normalize(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for component in abs.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolves the Hotpot Home root: `$HOTPOT_HOME` when set (non-empty), else `$HOME/.hotpot`.
pub fn resolve_hotpot_home() -> Result<PathBuf, PlanError> {
    if let Some(raw) = env::var_os("HOTPOT_HOME")
        && !raw.is_empty()
    {
        let path = PathBuf::from(raw);
        return Ok(if path.is_absolute() {
            path
        } else {
            env::current_dir().unwrap_or_default().join(path)
        });
    }
    match env::var_os("HOME") {
        Some(raw) if !raw.is_empty() => Ok(PathBuf::from(raw).join(".hotpot")),
        _ => Err(PlanError::new(
            "home-unresolved",
            "cannot resolve Hotpot Home: neither HOTPOT_HOME nor HOME is set",
        )),
    }
}

/// Finds an executable `bin` on `PATH` without invoking it (read-only discovery).
pub fn find_on_path(bin: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let raw = env::var_os("PATH")?;
    for dir in env::split_paths(&raw) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(bin);
        if let Ok(meta) = fs::metadata(&candidate)
            && meta.is_file()
            && meta.permissions().mode() & 0o111 != 0
        {
            return Some(candidate);
        }
    }
    None
}

/// The deterministic `hotpot.json` marker content for the base plan: the short digest of the
/// rendered role contracts and the runtime opt-in state (the Runtime Coordinator is opt-in;
/// `runtime/v1/` stays preserve-only either way).
pub fn hotpot_marker_contents(contracts: &RenderedContracts, runtime_opt_in: bool) -> String {
    serde_json::to_string_pretty(&json!({
        "marker": "hotpot",
        "version": 1,
        "runtime_opt_in": runtime_opt_in,
        "role_contract_digest": contracts.digest,
    }))
    .unwrap()
        + "\n"
}

/// Runtime opt-in state read from the environment: `HOTPOT_RUNTIME_OPT_IN` set to a truthy
/// value (`1`, `true`, `yes`, `on`, case-insensitive) opts this Home into the Runtime
/// Coordinator; anything else (including unset) stays opted out.
pub fn runtime_opt_in_from_env() -> bool {
    env::var("HOTPOT_RUNTIME_OPT_IN")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Finds role-owned credential files under `<home>/roles/` so `hotpot remove` can warn
/// before deleting them.
pub fn find_role_credential_files(home: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![home.join("roles")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && CREDENTIAL_FILE_NAMES.contains(&name)
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// True when the preserve-only runtime state tree holds at least one entry (active runtime).
pub fn runtime_state_active(home: &Path) -> bool {
    fs::read_dir(home.join(RUNTIME_STATE_DIR))
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// Every path under `home` (children first) followed by `home` itself — the deletion list
/// printed by `hotpot remove`.
pub fn removal_paths(home: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() && !path.is_symlink() {
                walk(&path, out);
            }
            out.push(path);
        }
    }
    let mut out = Vec::new();
    walk(home, &mut out);
    out.push(home.to_path_buf());
    out
}

/// Refuses to tear down implausible roots (the filesystem root or `$HOME` itself).
pub fn guard_removal_root(home: &Path) -> Result<(), PlanError> {
    let home = normalize(home);
    if home == Path::new("/") {
        return Err(PlanError::new(
            "remove-refused",
            "refusing to remove the filesystem root",
        ));
    }
    if let Some(raw) = env::var_os("HOME")
        && !raw.is_empty()
        && normalize(&PathBuf::from(raw)) == home
    {
        return Err(PlanError::new(
            "remove-refused",
            format!("refusing to remove HOME itself ({})", home.display()),
        ));
    }
    Ok(())
}

/// Builds the base Hotpot Home plan: the home skeleton directories, the nine Role Homes
/// (vendor configs + additive instruction artifacts + Capability Hygiene closures, via
/// [`role_homes::seat_plan_items`]), `AUTHENTICATION.md`, and the `hotpot.json` marker
/// carrying the role-contract digest and runtime opt-in state. Skill projections (the
/// built-in `hotpot-project-onboarding` skill, the per-agent Lead aggregates with their Role
/// Home wirings, the Peer hygiene projections, and the `skill-projections/manifest.json`
/// fence source) are generated from the read-only operator skill roots under
/// `operator_home` (`~/.codex/skills`, `~/.claude/skills`, `~/.pi/agent/skills`); collision
/// exclusions ride along as plan notes. `runtime/` is never part of a plan (preserve-only);
/// `hc` profile changes are only planned when `hc` is discoverable on `PATH`. Fails closed
/// when the operator's Codex model catalog cannot be produced scrubbed
/// ([`hygiene::operator_codex_catalog`]).
pub fn base_home_plan(
    home: &Path,
    hotpot_bin: &Path,
    contracts: &RenderedContracts,
    runtime_opt_in: bool,
    hc_available: bool,
    operator_home: &Path,
) -> Result<Plan, PlanError> {
    let codex_catalog = hygiene::scrubbed_catalog_contents(&hygiene::operator_codex_catalog()?);
    let pi_models = role_homes::operator_pi_models_contents(operator_home)?;
    let mut items: Vec<PlanItem> = Vec::new();
    for agent in AGENTS {
        for role in ROLES {
            items.push(PlanItem::Dir(role_homes::role_home_dir(home, agent, role)));
            items.extend(role_homes::seat_plan_items(
                home,
                hotpot_bin,
                agent,
                role,
                contracts.for_role(role),
                &codex_catalog,
                pi_models.as_deref(),
            ));
        }
    }
    items.push(PlanItem::Dir(home.join("skills")));
    for agent in AGENTS {
        items.push(PlanItem::Dir(
            home.join("skill-projections").join(agent).join("lead"),
        ));
    }
    let skill_projection = crate::skills::plan_skill_projection(home, operator_home);
    items.extend(skill_projection.items);
    items.push(PlanItem::File {
        path: home.join("hotpot.json"),
        contents: hotpot_marker_contents(contracts, runtime_opt_in),
    });
    items.push(PlanItem::File {
        path: home.join("AUTHENTICATION.md"),
        contents: role_homes::authentication_md(),
    });
    if hc_available {
        items.push(PlanItem::HcProfileSet {
            profile: "hotpot".to_string(),
            key: "hotpot.v1".to_string(),
            value: "true".to_string(),
        });
    }
    let mut notes = skill_projection.collision_notes;
    if pi_models.is_none() {
        notes.push(format!(
            "note: no operator Pi models.json at {}; Pi Role Homes are provisioned without a \
             models config (ZCode-via-pi routing stays inactive until one exists)",
            operator_home.join(".pi").join("agent").join("models.json").display()
        ));
    }
    Ok(Plan {
        home: home.to_path_buf(),
        items,
        notes,
    })
}

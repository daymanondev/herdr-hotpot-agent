//! Desired-state provisioner engine over Hotpot Home.
//!
//! The provisioner layer is a stateless desired-state generator: `hotpot setup` builds a
//! [`Plan`] (planned directories, files, symlinks, and `hc` profile changes), prints it as a
//! dry-run by default, and only materializes it with `--apply`. All writes are strictly
//! confined to Hotpot Home (`HOTPOT_HOME`, defaulting to `~/.hotpot`); operator homes such as
//! `~/.codex`, `~/.claude`, and `~/.pi/agent` are read-only inputs, and `runtime/v1/` (the
//! runtime coordinator's append-only state) is preserve-only — never planned, never written.

use serde_json::json;
use std::env;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

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

    /// Prints the dry-run report without mutating anything.
    pub fn print_dry_run(&self, out: &mut impl Write) -> std::io::Result<()> {
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
    /// Not yet implemented: driven by the next TDD cycle.
    pub fn apply(&self, _out: &mut impl Write) -> Result<(), PlanError> {
        self.validate()?;
        Err(PlanError::new(
            "apply-not-implemented",
            "setup --apply is not implemented yet",
        ))
    }
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

/// The deterministic `hotpot.json` marker content for the base plan.
pub fn hotpot_marker_contents() -> String {
    serde_json::to_string_pretty(&json!({
        "marker": "hotpot",
        "version": 1
    }))
    .unwrap()
        + "\n"
}

/// Builds the base Hotpot Home plan: the home skeleton directories plus the `hotpot.json`
/// marker. `runtime/` is never part of a plan (preserve-only); `hc` profile changes are only
/// planned when `hc` is discoverable on `PATH`. Later tickets plug Role Home, hygiene, and
/// skill-projection generators into this same [`Plan`].
pub fn base_home_plan(home: &Path, hc_available: bool) -> Plan {
    let mut items: Vec<PlanItem> = Vec::new();
    for agent in AGENTS {
        for role in ROLES {
            items.push(PlanItem::Dir(home.join("roles").join(agent).join(role)));
        }
    }
    items.push(PlanItem::Dir(home.join("skills")));
    for agent in AGENTS {
        items.push(PlanItem::Dir(
            home.join("skill-projections").join(agent).join("lead"),
        ));
    }
    items.push(PlanItem::File {
        path: home.join("hotpot.json"),
        contents: hotpot_marker_contents(),
    });
    if hc_available {
        items.push(PlanItem::HcProfileSet {
            profile: "hotpot".to_string(),
            key: "hotpot.v1".to_string(),
            value: "true".to_string(),
        });
    }
    Plan {
        home: home.to_path_buf(),
        items,
    }
}

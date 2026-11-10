//! Skill projections over operator skill trees, the built-in `hotpot-project-onboarding`
//! skill, and the projection manifest that feeds the runtime skill fence.
//!
//! `hotpot setup` reads the operator's per-vendor skill roots (read-only inputs under the
//! operator's `HOME`: `~/.codex/skills`, `~/.claude/skills`, `~/.pi/agent/skills`) and
//! projects them into Hotpot Home:
//!
//! - **Lead aggregate** (`skill-projections/<agent>/lead/`): every non-colliding,
//!   non-orchestration operator skill copied in full (executable resources included), plus
//!   the built-in proposal-first [`ONBOARDING_SKILL_NAME`] skill. Each Lead Role Home wires
//!   its `skills` symlink to this aggregate.
//! - **Peer projection** (`roles/<agent>/peer/skills/`): the same skill selection minus
//!   executable resources ([`PEER_WITHHELD_RESOURCE_DIRS`]) and minus any skill whose
//!   artifacts reference the Lead-only [`PEER_BOUNDARY_MARKER`] document.
//! - **Projection manifest** (`skill-projections/manifest.json`): the Lead/Peer skill
//!   allowlists plus the collision report. `hotpot hook` derives its runtime skill fence
//!   from this manifest — never from a hardcoded list.
//!
//! Collisions (the same skill name provided by more than one operator root) cannot be
//! attributed to a single source deterministically, so they are excluded from every
//! projection and reported in the setup plan.

use crate::provisioner::{AGENTS, PlanItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The built-in, proposal-first onboarding skill projected into every Lead aggregate.
pub const ONBOARDING_SKILL_NAME: &str = "hotpot-project-onboarding";

/// Skill-name prefixes that mark orchestration surfaces; such skills are projected out of
/// every Role Home so no Seat can open a second control plane (`GLOSSARY.md` Capability
/// Hygiene). The built-in onboarding skill is added explicitly after this filter.
pub const ORCHESTRATION_SKILL_PREFIXES: &[&str] = &["hotpot", "paseo", "factory"];

/// Executable resource directory names withheld entirely from Peer projections (matched at
/// any depth); the Lead aggregate keeps them.
pub const PEER_WITHHELD_RESOURCE_DIRS: &[&str] = &["plugins", "hooks", "commands", "prompts"];

/// The boundary marker no Peer artifact may contain: the repository's workflow protocol is
/// Lead-only (`GLOSSARY.md` WORKSPACE_PROTOCOL), so a skill referencing it never reaches a
/// Peer Role Home.
pub const PEER_BOUNDARY_MARKER: &str = "WORKSPACE_PROTOCOL";

/// The operator (read-only) skill root of one agent family under the operator's `HOME`.
fn operator_skill_root(operator_home: &Path, agent: &str) -> PathBuf {
    match agent {
        "codex" => operator_home.join(".codex").join("skills"),
        "claude" => operator_home.join(".claude").join("skills"),
        "pi" => operator_home.join(".pi").join("agent").join("skills"),
        other => unreachable!("operator_skill_root called for unknown agent {other}"),
    }
}

/// True when `name` marks an orchestration skill (hotpot/paseo/factory prefix).
fn is_orchestration_skill(name: &str) -> bool {
    ORCHESTRATION_SKILL_PREFIXES
        .iter()
        .any(|prefix| name.to_lowercase().starts_with(prefix))
}

/// The built-in `hotpot-project-onboarding` skill body (proposal-first: an Evidence Map over
/// read-only repository inspection, and a `WORKSPACE_PROTOCOL.md` draft written only upon
/// explicit Human confirmation).
const ONBOARDING_SKILL_MD: &str = r#"---
name: hotpot-project-onboarding
description: Proposal-first repository onboarding for the Hotpot Lead seat - build an Evidence Map from repository manifests, scripts, tests, and docs, then draft WORKSPACE_PROTOCOL.md only after explicit Human confirmation.
---

# Hotpot Project Onboarding (proposal-first)

Onboard a repository's workflow policy without inventing it: every step below is read-only
until the Human explicitly confirms the draft. You are the Lead; the resulting protocol is
yours alone and is never shown to Peers.

## Steps

1. **Inspect (read-only).** Survey the repository's manifests (`Cargo.toml`,
   `package.json`, `pyproject.toml`, ...), scripts (`scripts/`, `Makefile`), test suites,
   and docs (`AGENTS.md`, `docs/`, ADRs). Modify nothing in this step.
2. **Evidence Map.** Present an Evidence Map with one row per claim: the claim, the exact
   file path that proves it, and the line or section inside that file. Claims you cannot
   evidence are marked `unverified` — never guessed or silently dropped.
3. **Draft proposal.** Propose a draft `WORKSPACE_PROTOCOL.md` covering the repository's
   workflow policy (topology, model routing, proof gates, review rules), with every section
   traced back to Evidence Map rows.
4. **Explicit Human confirmation before write.** Never write `WORKSPACE_PROTOCOL.md` until
   the Human explicitly confirms the draft in conversation. Write exactly the confirmed
   draft to the repository root. Any revision request returns to step 3; silence is not
   confirmation.

## Laws

- Proposal-first: the step-4 write happens only after explicit Human confirmation.
- Evidence beats prose: no protocol section survives without an Evidence Map row.
- The protocol is Lead-only: never paste its content into Peer briefs or artifacts.
"#;

/// One collision entry of the projection manifest: a skill name provided by more than one
/// operator root (the roots are absolute paths, in deterministic sorted order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestCollision {
    pub skill: String,
    pub roots: Vec<String>,
}

/// The projection manifest written to `skill-projections/manifest.json` under Hotpot Home.
/// `lead_skills`/`peer_skills` are the runtime skill-fence allowlists; `collisions` records
/// the deterministically excluded skill names and the roots that fought over them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionManifest {
    pub version: u32,
    pub lead_skills: Vec<String>,
    pub peer_skills: Vec<String>,
    pub collisions: Vec<ManifestCollision>,
}

/// Loads the projection manifest from `<state_dir>/skill-projections/manifest.json`
/// (`None` when the state dir or manifest is absent or unparseable — callers fail closed).
pub fn load_projection_manifest(state_dir: &Path) -> Option<ProjectionManifest> {
    let text = fs::read_to_string(state_dir.join("skill-projections").join("manifest.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// A walked skill tree: relative sub-directories and UTF-8 file contents, both sorted
/// deterministically. Symlinks and non-UTF-8 files are skipped (skills are text-first; a
/// symlinked or binary resource is never projected).
struct TreeWalk {
    dirs: Vec<PathBuf>,
    files: Vec<(PathBuf, String)>,
}

fn walk_tree(root: &Path) -> TreeWalk {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let dir = if rel.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                continue;
            }
            let child_rel = rel.join(entry.file_name());
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                dirs.push(child_rel.clone());
                stack.push(child_rel);
            } else if let Ok(contents) = fs::read_to_string(entry.path()) {
                files.push((child_rel, contents));
            }
        }
    }
    dirs.sort();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    TreeWalk { dirs, files }
}

/// Plan items copying a walked tree under `dest`, skipping any path with a segment in
/// `withheld_dirs` (empty for the full Lead copy).
fn copy_items(dest: &Path, walk: &TreeWalk, withheld_dirs: &[&str]) -> Vec<PlanItem> {
    let withheld = |rel: &Path| {
        withheld_dirs
            .iter()
            .any(|name| rel.iter().any(|seg| seg == *name))
    };
    let mut items = Vec::new();
    for rel in &walk.dirs {
        if !withheld(rel) {
            items.push(PlanItem::Dir(dest.join(rel)));
        }
    }
    for (rel, contents) in &walk.files {
        if !withheld(rel) {
            items.push(PlanItem::File {
                path: dest.join(rel),
                contents: contents.clone(),
            });
        }
    }
    items
}

/// Scans every operator skill root: skill name -> the roots providing it. A directory
/// counts as a skill when it directly contains a `SKILL.md`.
fn scan_operator_skills(operator_home: &Path) -> BTreeMap<String, Vec<PathBuf>> {
    let mut found: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for agent in AGENTS {
        let root = operator_skill_root(operator_home, agent);
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && !path.is_symlink()
                && path.join("SKILL.md").is_file()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                found.entry(name.to_string()).or_default().push(path);
            }
        }
    }
    for roots in found.values_mut() {
        roots.sort();
    }
    found
}

/// The skill-projection slice of one `hotpot setup` plan: plan items to materialize, the
/// manifest to publish, and the collision notes the plan reports.
pub struct SkillProjectionPlan {
    pub items: Vec<PlanItem>,
    pub manifest: ProjectionManifest,
    pub collision_notes: Vec<String>,
}

/// Builds the full skill-projection plan over `home` from the operator's skill roots under
/// `operator_home` (read-only): the built-in onboarding asset, the per-agent Lead aggregates
/// and their Role Home wirings, the per-agent Peer hygiene projections, and the projection
/// manifest.
pub fn plan_skill_projection(home: &Path, operator_home: &Path) -> SkillProjectionPlan {
    let discovered = scan_operator_skills(operator_home);

    let collisions: Vec<ManifestCollision> = discovered
        .iter()
        .filter(|(_, roots)| roots.len() > 1)
        .map(|(name, roots)| ManifestCollision {
            skill: name.clone(),
            roots: roots
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
        })
        .collect();

    let mut items: Vec<PlanItem> = vec![PlanItem::File {
        path: home
            .join("skills")
            .join(ONBOARDING_SKILL_NAME)
            .join("SKILL.md"),
        contents: ONBOARDING_SKILL_MD.to_string(),
    }];

    let mut lead_skills: Vec<String> = Vec::new();
    let mut peer_skills: Vec<String> = Vec::new();

    for (name, roots) in &discovered {
        // Colliding and orchestration-prefixed skills are projected out everywhere.
        if roots.len() > 1 || is_orchestration_skill(name) {
            continue;
        }
        let walk = walk_tree(&roots[0]);

        // Lead keeps full domain awareness: the whole tree, executable resources included.
        lead_skills.push(name.clone());
        for agent in AGENTS {
            let dest = home
                .join("skill-projections")
                .join(agent)
                .join("lead")
                .join(name);
            items.extend(copy_items(&dest, &walk, &[]));
        }

        // Peer hygiene: a skill whose artifacts reference the Lead-only boundary document
        // never reaches a Peer Role Home.
        if walk
            .files
            .iter()
            .any(|(_, contents)| contents.contains(PEER_BOUNDARY_MARKER))
        {
            continue;
        }
        peer_skills.push(name.clone());
        for agent in AGENTS {
            let dest = home
                .join("roles")
                .join(agent)
                .join("peer")
                .join("skills")
                .join(name);
            items.extend(copy_items(&dest, &walk, PEER_WITHHELD_RESOURCE_DIRS));
        }
    }

    // Wire the onboarding skill into every Lead aggregate and the aggregate into every Lead
    // Role Home; keep a stable (even when empty) Peer skills directory per agent family.
    lead_skills.push(ONBOARDING_SKILL_NAME.to_string());
    for agent in AGENTS {
        items.push(PlanItem::Symlink {
            path: home
                .join("skill-projections")
                .join(agent)
                .join("lead")
                .join(ONBOARDING_SKILL_NAME),
            target: PathBuf::from(format!("../../../skills/{ONBOARDING_SKILL_NAME}")),
        });
        items.push(PlanItem::Dir(
            home.join("roles").join(agent).join("peer").join("skills"),
        ));
        items.push(PlanItem::Symlink {
            path: home.join("roles").join(agent).join("lead").join("skills"),
            target: PathBuf::from(format!("../../../skill-projections/{agent}/lead")),
        });
    }

    lead_skills.sort();
    peer_skills.sort();
    let manifest = ProjectionManifest {
        version: 1,
        lead_skills,
        peer_skills,
        collisions,
    };
    items.push(PlanItem::File {
        path: home.join("skill-projections").join("manifest.json"),
        contents: serde_json::to_string_pretty(&manifest).unwrap() + "\n",
    });

    let collision_notes = manifest
        .collisions
        .iter()
        .map(|c| {
            format!(
                "collision: skill `{}` is provided by multiple operator roots ({}); excluded from every skill projection",
                c.skill,
                c.roots.join(", ")
            )
        })
        .collect();

    SkillProjectionPlan {
        items,
        manifest,
        collision_notes,
    }
}

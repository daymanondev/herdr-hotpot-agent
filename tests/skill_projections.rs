//! CLI-level integration tests for skill projections, the `hotpot-project-onboarding` skill,
//! Peer hygiene, and the WORKSPACE_PROTOCOL visibility boundary (ticket 04 of the Hotpot
//! Foundation spec).
//!
//! Seam (per the ticket's "Testing seam" and `.scratch/hotpot-foundation/spec.md` "Testing
//! Decisions → Highest-Level Testing Seam"): invoke the `hotpot` binary with `HOTPOT_HOME` and
//! `HOME` pointed at temp dirs, plant fixture operator skill trees under the fake operator
//! homes, then assert directory trees, symlinks, file contents under the temp home plus
//! `hotpot hook` JSON verdicts. Zero network; the operator homes are read-only inputs.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Best-effort self-cleaning temp dir (the crate deliberately has no test-only dependencies).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hotpot-skills-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The operator skill roots `hotpot setup` reads (read-only inputs), keyed by agent family.
fn operator_skill_roots(fake_home: &Path) -> [(&'static str, PathBuf); 3] {
    [
        ("codex", fake_home.join(".codex/skills")),
        ("claude", fake_home.join(".claude/skills")),
        ("pi", fake_home.join(".pi/agent/skills")),
    ]
}

/// Plants one fixture operator skill (`<root>/<name>/SKILL.md` plus optional extra files given
/// as slash-separated relative paths) and returns the skill directory.
fn plant_skill(root: &Path, name: &str, extra_files: &[(&str, &str)]) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: fixture operator skill {name}\n---\n\n# {name}\n\nFixture body for {name}.\n"),
    )
    .unwrap();
    for (rel, contents) in extra_files {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    dir
}

/// The standard fixture set from acceptance criterion 1 across the operator roots.
fn plant_standard_fixture(fake_home: &Path) {
    let [codex, claude, pi] = {
        let roots = operator_skill_roots(fake_home);
        [roots[0].1.clone(), roots[1].1.clone(), roots[2].1.clone()]
    };
    for (root, names) in [
        (&codex, vec!["tdd", "codebase-design", "hotpot-internal"]),
        (&claude, vec!["paseo-room", "factory-skills"]),
        (&pi, vec![]),
    ] {
        for name in names {
            plant_skill(root, name, &[]);
        }
    }
}

/// Minimal PATH so the real operator environment can never leak into these tests.
fn sandbox_path() -> String {
    "/usr/bin:/bin:/usr/sbin:/sbin".to_string()
}

fn run_hotpot(
    home: &Path,
    fake_home: &Path,
    args: &[&str],
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args(args)
        .env("HOTPOT_HOME", home)
        .env("HOME", fake_home)
        .env("PATH", sandbox_path())
        .output()
        .unwrap()
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn run_setup_apply(home: &Path, fake_home: &Path) -> Output {
    let out = run_hotpot(home, fake_home, &["setup", "--apply"]);
    assert!(
        out.status.success(),
        "setup --apply must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    out
}

/// Feeds `payload` to `hotpot hook --role <role> --state-dir <state_dir>` and returns the
/// parsed JSON verdict on stdout.
fn run_hook(role: &str, state_dir: &Path, payload: &str) -> serde_json::Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args(["hook", "--role", role, "--state-dir"])
        .arg(state_dir)
        .env("PATH", sandbox_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "hook must exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("hook stdout must be JSON: {e}\n{}", String::from_utf8_lossy(&out.stdout)))
}

/// The `hookSpecificOutput.permissionDecision` of a hook verdict.
fn decision(verdict: &serde_json::Value) -> String {
    verdict["hookSpecificOutput"]["permissionDecision"]
        .as_str()
        .unwrap_or_else(|| panic!("verdict carries permissionDecision: {verdict}"))
        .to_string()
}

/// Names of the direct children of `dir`.
fn child_names(dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        names.insert(
            entry
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
    }
    names
}

/// Every file under `root` (recursive, never following symlinks), as relative paths.
fn relative_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Every directory name anywhere under `root` (recursive, never following symlinks).
fn directory_names_under(root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                names.insert(
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
                stack.push(path);
            }
        }
    }
    names
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1: Lead aggregate & Peer projection contents.
// ---------------------------------------------------------------------------

#[test]
fn lead_aggregate_and_peer_projection_carry_exactly_the_projected_skills() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    plant_standard_fixture(fake_home.path());

    run_setup_apply(home.path(), fake_home.path());

    for agent in ["codex", "claude", "pi"] {
        // Lead aggregate: the two non-orchestration operator skills plus the built-in
        // onboarding skill; hotpot-internal / paseo-room / factory-skills are projected out.
        let aggregate = home.path().join("skill-projections").join(agent).join("lead");
        assert_eq!(
            child_names(&aggregate),
            BTreeSet::from([
                "tdd".to_string(),
                "codebase-design".to_string(),
                "hotpot-project-onboarding".to_string(),
            ]),
            "the {agent} Lead aggregate must contain exactly the projected skills"
        );

        // Peer projection: no orchestration-prefixed skills, no onboarding skill.
        let peer_skills = home.path().join("roles").join(agent).join("peer").join("skills");
        assert_eq!(
            child_names(&peer_skills),
            BTreeSet::from(["tdd".to_string(), "codebase-design".to_string()]),
            "the {agent} Peer projection must contain exactly the hygiene-filtered skills"
        );
    }

    // The projected skills are real content, not empty stubs.
    let skill_md = home
        .path()
        .join("skill-projections/claude/lead/tdd/SKILL.md");
    assert!(
        fs::read_to_string(&skill_md)
            .unwrap()
            .contains("fixture operator skill tdd"),
        "the aggregate must carry the operator skill's content: {}",
        skill_md.display()
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 2: executable resources withheld from Peer, kept for Lead.
// ---------------------------------------------------------------------------

#[test]
fn peer_projection_withholds_executable_resources_but_lead_keeps_them() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let roots = operator_skill_roots(fake_home.path());
    plant_skill(
        &roots[0].1,
        "codebase-design",
        &[
            ("plugins/run.sh", "# plugin resource\n"),
            ("hooks/hook.js", "// hook resource\n"),
            ("commands/cmd.md", "command resource\n"),
            ("prompts/prompt.md", "prompt resource\n"),
            ("reference/patterns.md", "plain reference file\n"),
        ],
    );

    run_setup_apply(home.path(), fake_home.path());

    for agent in ["codex", "claude", "pi"] {
        let peer_home = home.path().join("roles").join(agent).join("peer");
        let dir_names = directory_names_under(&peer_home);
        for resource in ["plugins", "hooks", "commands", "prompts"] {
            assert!(
                !dir_names.contains(resource),
                "`{resource}` must not exist anywhere under the {agent} Peer Role Home: {:?}",
                dir_names
            );
        }
        // The non-executable reference file survives the hygiene filter.
        assert!(
            peer_home
                .join("skills/codebase-design/reference/patterns.md")
                .exists(),
            "plain skill resources must survive Peer hygiene"
        );
        assert!(
            peer_home.join("skills/codebase-design/SKILL.md").exists(),
            "the skill body itself must survive Peer hygiene"
        );
    }

    // The Lead aggregate keeps the skill with its executable resources intact.
    for agent in ["codex", "claude", "pi"] {
        let aggregate = home
            .path()
            .join("skill-projections")
            .join(agent)
            .join("lead")
            .join("codebase-design");
        assert!(aggregate.join("SKILL.md").exists(), "Lead keeps the skill body");
        for resource in ["plugins/run.sh", "hooks/hook.js", "commands/cmd.md", "prompts/prompt.md"] {
            assert!(
                aggregate.join(resource).exists(),
                "Lead aggregate must keep the executable resource {resource}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 3: deterministic collision exclusion, reported by the dry run.
// ---------------------------------------------------------------------------

#[test]
fn colliding_operator_skills_are_excluded_and_reported_in_the_dry_run() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    plant_standard_fixture(fake_home.path());
    let roots = operator_skill_roots(fake_home.path());
    let codex_root = plant_skill(&roots[0].1, "foo", &[("extra.txt", "from codex\n")]);
    let claude_root = plant_skill(&roots[1].1, "foo", &[("extra.txt", "from claude\n")]);

    let dry = run_hotpot(home.path(), fake_home.path(), &["setup"]);
    assert!(dry.status.success(), "dry-run must exit 0: {}", stderr_of(&dry));
    let stdout = stdout_of(&dry);
    assert!(
        stdout.to_lowercase().contains("collision"),
        "dry-run stdout must report the collision:\n{stdout}"
    );
    assert!(
        stdout.contains("foo"),
        "the collision report must name the colliding skill:\n{stdout}"
    );
    assert!(
        stdout.contains(codex_root.display().to_string().as_str())
            && stdout.contains(claude_root.display().to_string().as_str()),
        "the collision report must name both operator roots:\n{stdout}"
    );

    run_setup_apply(home.path(), fake_home.path());
    for agent in ["codex", "claude", "pi"] {
        let aggregate = home.path().join("skill-projections").join(agent).join("lead");
        assert!(
            !child_names(&aggregate).contains("foo"),
            "the colliding skill `foo` must be excluded from the {agent} Lead aggregate"
        );
        let peer_skills = home.path().join("roles").join(agent).join("peer").join("skills");
        assert!(
            !child_names(&peer_skills).contains("foo"),
            "the colliding skill `foo` must be excluded from the {agent} Peer projection"
        );
    }

    // Determinism: two independent runs plan byte-identical aggregate trees for `foo`'s
    // exclusion (the collision never resolves to a root-of-the-day).
    let home_b = TempDir::new("homeb");
    let dry_b = run_hotpot(home_b.path(), fake_home.path(), &["setup"]);
    let foo_lines_a: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains("collision") && l.contains("foo"))
        .collect();
    let stdout_b = stdout_of(&dry_b);
    let foo_lines_b: Vec<&str> = stdout_b
        .lines()
        .filter(|l| l.contains("collision") && l.contains("foo"))
        .collect();
    assert_eq!(
        foo_lines_a, foo_lines_b,
        "the collision report must be deterministic across runs"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4: built-in onboarding skill asset, wired into the Lead skills config.
// ---------------------------------------------------------------------------

#[test]
fn onboarding_skill_exists_carries_its_rules_and_is_wired_into_the_lead_aggregate() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    plant_standard_fixture(fake_home.path());

    run_setup_apply(home.path(), fake_home.path());

    let asset = home.path().join("skills/hotpot-project-onboarding/SKILL.md");
    let text = fs::read_to_string(&asset)
        .unwrap_or_else(|e| panic!("built-in onboarding skill asset must exist: {e}"));
    assert!(
        text.contains("Evidence Map"),
        "the onboarding skill must contain the evidence-map step:\n{text}"
    );
    assert!(
        text.to_lowercase().contains("explicit human confirmation"),
        "the onboarding skill must contain the explicit-Human-confirmation-before-write rule:\n{text}"
    );

    for agent in ["codex", "claude", "pi"] {
        // Wired into the Lead skills config: the seat's skills directory is a symlink whose
        // target is the managed aggregate, and the onboarding skill is reachable through it.
        let lead_skills = home.path().join("roles").join(agent).join("lead").join("skills");
        assert!(
            lead_skills.is_symlink(),
            "{agent} Lead skills directory must be a symlink to the managed aggregate"
        );
        let target = fs::read_link(&lead_skills).unwrap();
        let resolved = if target.is_absolute() {
            target.clone()
        } else {
            lead_skills.parent().unwrap().join(&target)
        };
        let canonical = fs::canonicalize(&resolved).unwrap();
        let aggregate = fs::canonicalize(
            home.path().join("skill-projections").join(agent).join("lead"),
        )
        .unwrap();
        assert_eq!(
            canonical, aggregate,
            "{agent} Lead skills symlink must resolve into the managed aggregate"
        );

        let through = home
            .path()
            .join(format!("roles/{agent}/lead/skills/hotpot-project-onboarding/SKILL.md"));
        assert_eq!(
            fs::read_to_string(&through).unwrap(),
            text,
            "the onboarding skill must be readable through the Lead skills wiring"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5: WORKSPACE_PROTOCOL visibility boundary.
// ---------------------------------------------------------------------------

#[test]
fn workspace_protocol_is_lead_only_across_every_peer_artifact() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    plant_standard_fixture(fake_home.path());
    // An operator skill that references WORKSPACE_PROTOCOL must not reach Peer either.
    let roots = operator_skill_roots(fake_home.path());
    plant_skill(
        &roots[2].1,
        "protocol-heavy",
        &[("notes.md", "See WORKSPACE_PROTOCOL.md for the rules.\n")],
    );

    run_setup_apply(home.path(), fake_home.path());

    // Lead contract: references reading the root protocol with the built-in fallback.
    for agent in ["codex", "claude", "pi"] {
        let lead_contract = match agent {
            "codex" => {
                let config = fs::read_to_string(
                    home.path().join("roles/codex/lead/config.json"),
                )
                .unwrap();
                let value: serde_json::Value = serde_json::from_str(&config).unwrap();
                value["developer_instructions"].as_str().unwrap().to_string()
            }
            "claude" => fs::read_to_string(
                home.path().join("roles/claude/lead/CLAUDE.md"),
            )
            .unwrap(),
            _ => fs::read_to_string(
                home.path().join("roles/pi/lead/APPEND_SYSTEM.md"),
            )
            .unwrap(),
        };
        assert!(
            lead_contract.contains("WORKSPACE_PROTOCOL.md"),
            "the {agent} Lead contract must reference reading the root WORKSPACE_PROTOCOL.md"
        );
        assert!(
            lead_contract.contains("built-in operating baseline"),
            "the {agent} Lead contract must name the built-in fallback"
        );
    }

    // Peer boundary: no file under any Peer Role Home references WORKSPACE_PROTOCOL.
    for agent in ["codex", "claude", "pi"] {
        let peer_home = home.path().join("roles").join(agent).join("peer");
        for file in relative_files(&peer_home) {
            let bytes = fs::read(&file).unwrap();
            assert!(
                !bytes
                    .windows("WORKSPACE_PROTOCOL".len())
                    .any(|w| w == b"WORKSPACE_PROTOCOL"),
                "Peer artifact {} must not contain WORKSPACE_PROTOCOL",
                file.display()
            );
        }
    }

    // The protocol-heavy skill itself is kept out of the Peer projection entirely (its
    // content references the boundary document) while Lead keeps it.
    for agent in ["codex", "claude", "pi"] {
        assert!(
            !home
                .path()
                .join(format!("roles/{agent}/peer/skills/protocol-heavy"))
                .exists(),
            "a skill referencing WORKSPACE_PROTOCOL must not reach the {agent} Peer projection"
        );
        assert!(
            home.path()
                .join(format!("skill-projections/{agent}/lead/protocol-heavy/notes.md"))
                .exists(),
            "Lead keeps full domain awareness of the protocol-heavy skill"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 6: manifest-derived runtime skill fence through `hotpot hook`.
// ---------------------------------------------------------------------------

#[test]
fn hook_skill_verdicts_derive_from_the_projection_manifest() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    plant_standard_fixture(fake_home.path());
    run_setup_apply(home.path(), fake_home.path());

    // Lead: a projected-in skill is allowed, a projected-out skill is denied.
    let allow = run_hook(
        "lead",
        home.path(),
        r#"{"tool_name":"Skill","tool_input":{"skill":"codebase-design"}}"#,
    );
    assert_eq!(decision(&allow), "allow", "Lead codebase-design verdict: {allow}");

    let deny = run_hook(
        "lead",
        home.path(),
        r#"{"tool_name":"Skill","tool_input":{"skill":"hotpot-internal"}}"#,
    );
    assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        deny["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap_or_default()
            .contains("lead-skill-fence"),
        "the denial reason must carry the fence code: {deny}"
    );

    // Peer: projected-in skill allowed, projected-out skill denied.
    let peer_allow = run_hook(
        "peer",
        home.path(),
        r#"{"tool_name":"Skill","tool_input":{"skill":"tdd"}}"#,
    );
    assert_eq!(decision(&peer_allow), "allow", "Peer tdd verdict: {peer_allow}");

    let peer_deny = run_hook(
        "peer",
        home.path(),
        r#"{"tool_name":"Skill","tool_input":{"skill":"paseo-room"}}"#,
    );
    assert_eq!(decision(&peer_deny), "deny", "Peer paseo-room verdict: {peer_deny}");

    // The fence is enforced on read paths too (the same allowlist governs Skill loads): the
    // Lead skills wiring path of a projected-out skill is denied even though the path shape
    // is the seat's own skills directory.
    let read_deny = run_hook(
        "lead",
        home.path(),
        &format!(
            r#"{{"tool_name":"Read","tool_input":{{"file_path":"{}/roles/claude/lead/skills/factory-skills/SKILL.md"}}}}"#,
            home.path().display()
        ),
    );
    assert_eq!(decision(&read_deny), "deny", "Lead reading a projected-out skill: {read_deny}");
}

#[test]
fn hook_fails_closed_without_a_projection_manifest() {
    let fake_home = TempDir::new("fakehome");
    plant_standard_fixture(fake_home.path());
    let empty = TempDir::new("nostate");

    // No `setup --apply`: no projection manifest exists, so the old hardcoded Lead allowlist
    // must not silently keep granting skills.
    let verdict = run_hook(
        "lead",
        empty.path(),
        r#"{"tool_name":"Skill","tool_input":{"skill":"codebase-design"}}"#,
    );
    assert_eq!(
        decision(&verdict),
        "deny",
        "without a projection manifest the skill fence must fail closed: {verdict}"
    );
}

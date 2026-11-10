//! CLI-level integration tests for `hc` seat launching & the 1:1 Project topology (ticket 06
//! of the Hotpot Foundation spec).
//!
//! Seam (per `.scratch/hotpot-foundation/spec.md` "Testing Decisions → Highest-Level Testing
//! Seam" and ticket 06 "Testing seam"): invoke the `hotpot seat launch` binary against a
//! temporary `HOTPOT_HOME`, a temporary fake operator `HOME`, a real temporary git repository,
//! and a fake `hc` runner that records every `hc run` argv (labels and env ride on the argv)
//! to a log file and answers `hc ls --json` from a scripted terminal list. Assert the fake's
//! recorded argv/labels, stdout/stderr, and exit codes. Zero network; the operator homes
//! under the fake `HOME` are read-only inputs hotpot never writes.

mod common;

use common::{TempDir, init_git_repo};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// ---------------------------------------------------------------------------
// Independent sources of truth (spec ticket 06 / "Implementation Decisions →
// 1:1 Project Topology on Herdr"), deliberately written out as literals here
// instead of importing from `src/` so the tests can disagree.
// ---------------------------------------------------------------------------

/// Canonical label set carried by every hotpot-created `hc` terminal.
const LABEL_V1: &str = "hotpot.v1=true";
const LABEL_ROLE: &str = "hotpot.role=";
const LABEL_AGENT: &str = "hotpot.agent=";
const LABEL_PROJECT: &str = "hotpot.project=";
const LABEL_CORRELATION: &str = "hotpot.correlation=";
/// Peer-only labels; Supervisor/Lead launches must carry neither.
const LABEL_ASSIGNMENT: &str = "hotpot.assignment=";
const LABEL_PARENT: &str = "hotpot.parent=";

/// Pi's closed extension launch args (ticket 03 scope), pinned as literals here.
const PI_LAUNCH_ARGS: [&str; 6] = [
    "--no-extensions",
    "--extension",
    "builtin:mcp",
    "--extension",
    "builtin:codemode",
    "--no-approve",
];

/// Claude's canonical native-orchestration deny list as launch flags (ticket 03 scope).
const CLAUDE_DENY_TOOLS: [&str; 6] = ["Task", "Agent", "Workflow", "Coordination", "Cron", "Team"];

/// ASCII unit separator the fake `hc` uses between recorded argv elements.
const ARG_SEP: char = '\u{1f}';

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Minimal PATH so the real operator environment can never leak into these tests; the fake
/// `hc` and fake vendor binaries live in a prepended temp bin dir.
fn sandbox_path(fake_bin: &Path) -> String {
    format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", fake_bin.display())
}

/// One seat-launch fixture: temp Hotpot Home, temp fake operator home, temp bin dir with the
/// fake `hc` + fake vendor binaries, and the fake `hc` state dir (recorded argv log plus the
/// scripted `hc ls --json` terminal list).
struct LaunchFixture {
    home: TempDir,
    operator_home: TempDir,
    bin: TempDir,
    hc_state: TempDir,
    _repo_guard: TempDir,
    repo: PathBuf,
}

impl LaunchFixture {
    fn new(tag: &str) -> Self {
        let home = TempDir::new(&format!("seat-home-{tag}"));
        let operator_home = TempDir::new(&format!("seat-op-{tag}"));
        let bin = TempDir::new(&format!("seat-bin-{tag}"));
        let hc_state = TempDir::new(&format!("seat-hc-{tag}"));
        let (repo_guard, _head) = init_git_repo();

        // Empty terminal list by default: no existing seats, every launch creates one.
        fs::write(hc_state.path().join("list.json"), "[]\n").unwrap();
        write_fake_hc(bin.path());
        for vendor in ["codex", "claude", "pi"] {
            write_fake_vendor(bin.path(), vendor);
        }

        let repo = repo_guard.path().to_path_buf();
        Self {
            home,
            operator_home,
            bin,
            hc_state,
            _repo_guard: repo_guard,
            repo,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        run_hotpot(
            self.home.path(),
            self.operator_home.path(),
            self.bin.path(),
            self.hc_state.path(),
            args,
        )
    }

    /// `hotpot seat launch <role> --agent <agent> --project <repo>` (+ extra flags).
    fn launch(&self, role: &str, agent: &str, extra: &[&str]) -> Output {
        let mut args = vec!["seat", "launch", role, "--agent", agent, "--project"];
        args.push(self.repo.to_str().unwrap());
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// The argv of every recorded `hc run` invocation (each without the leading `run` word).
    fn create_invocations(&self) -> Vec<Vec<String>> {
        recorded_invocations(&self.hc_state.path().join("create.log"))
    }
}

/// The fake `hc`: `hc run ...` records its full argv as one unit-separator-separated line
/// appended to `<state>/create.log`; `hc ls --json` prints the scripted `<state>/list.json`;
/// anything else (e.g. `hc profile set ...` from `setup`) succeeds without side effects.
/// The state dir rides on `HOTPOT_FAKE_HC_STATE`, inherited from the `hotpot` process.
fn write_fake_hc(bin_dir: &Path) {
    let script = "\
#!/bin/sh\n\
state=\"$HOTPOT_FAKE_HC_STATE\"\n\
case \"$1\" in\n\
  ls) exec cat \"$state/list.json\" ;;\n\
  run) { for a in \"$@\"; do printf '\\037%s' \"$a\"; done; printf '\\n'; } >> \"$state/create.log\" ;;\n\
esac\n\
exit 0\n";
    let shim = bin_dir.join("hc");
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A fake vendor CLI binary: executable, never executed by these tests (the fake `hc` owns
/// the terminal), but resolvable on `PATH` by `hotpot seat launch`.
fn write_fake_vendor(bin_dir: &Path, vendor: &str) {
    let shim = bin_dir.join(vendor);
    fs::write(&shim, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_hotpot(
    home: &Path,
    fake_home: &Path,
    fake_bin: &Path,
    hc_state: &Path,
    args: &[&str],
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args(args)
        .env("HOTPOT_HOME", home)
        .env("HOME", fake_home)
        .env("HOTPOT_FAKE_HC_STATE", hc_state)
        .env("PATH", sandbox_path(fake_bin))
        .output()
        .unwrap()
}

/// Parses the fake `hc` create log into argv vectors (leading `run` word stripped).
fn recorded_invocations(log: &Path) -> Vec<Vec<String>> {
    let text = fs::read_to_string(log).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            line.trim_start_matches(ARG_SEP)
                .split(ARG_SEP)
                .filter(|s| !s.is_empty())
                .skip(1) // the `run` subcommand word
                .map(|s| s.to_string())
                .collect()
        })
        .collect()
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// True when `argv` carries the flag pair `["--label", value]`.
fn has_label(argv: &[String], value: &str) -> bool {
    argv.windows(2).any(|w| w[0] == "--label" && w[1] == value)
}

/// True when `argv` carries any `--label` whose value starts with `prefix`.
fn has_label_prefix(argv: &[String], prefix: &str) -> bool {
    argv.windows(2)
        .any(|w| w[0] == "--label" && w[1].starts_with(prefix))
}

/// True when `argv` carries the flag pair `["--env", value]`.
fn has_env(argv: &[String], value: &str) -> bool {
    argv.windows(2).any(|w| w[0] == "--env" && w[1] == value)
}

/// The canonical `<basename>-<sha1(gitRoot)[0..6]>` slug of `repo`, computed through
/// `shasum` (an SHA-1 implementation independent of the Rust engine's own).
fn expected_slug(repo: &Path) -> String {
    let canonical = repo.canonicalize().unwrap();
    let base = canonical.file_name().unwrap().to_str().unwrap();
    let out = Command::new("shasum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(canonical.to_string_lossy().as_bytes())?;
            child.wait_with_output()
        })
        .expect("shasum runs");
    let digest = String::from_utf8_lossy(&out.stdout);
    let hex: String = digest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    assert_eq!(hex.len(), 40, "shasum produced a full SHA-1 hex digest");
    format!("{base}-{}", &hex[..6])
}

/// Creates the Role Home directory `hotpot seat launch` requires (mirrors a prior
/// `hotpot setup --apply` without re-testing that subcommand here).
fn create_role_home(fixture: &LaunchFixture, agent: &str, role: &str) -> PathBuf {
    let role_home = fixture
        .home
        .path()
        .join("roles")
        .join(agent)
        .join(role);
    fs::create_dir_all(&role_home).unwrap();
    role_home
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1 — canonical labels on every launched seat
// ---------------------------------------------------------------------------

#[test]
fn supervisor_launch_attaches_full_canonical_label_set() {
    let fixture = LaunchFixture::new("sup-labels");
    create_role_home(&fixture, "codex", "supervisor");

    let out = fixture.launch("supervisor", "codex", &[]);
    assert!(
        out.status.success(),
        "supervisor launch must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    let invocations = fixture.create_invocations();
    assert_eq!(invocations.len(), 1, "exactly one hc terminal is created");
    let argv = &invocations[0];

    let slug = expected_slug(&fixture.repo);
    assert!(has_label(argv, LABEL_V1), "argv carries {LABEL_V1}: {argv:?}");
    assert!(
        has_label(argv, &format!("{LABEL_ROLE}supervisor")),
        "argv carries the supervisor role label: {argv:?}"
    );
    assert!(
        has_label(argv, &format!("{LABEL_AGENT}codex")),
        "argv carries the codex agent label: {argv:?}"
    );
    assert!(
        has_label(argv, &format!("{LABEL_PROJECT}{slug}")),
        "argv carries hotpot.project={slug} (basename + first 6 sha1 hex of the git root): {argv:?}"
    );
    assert!(
        has_label_prefix(argv, LABEL_CORRELATION),
        "argv carries a non-empty hotpot.correlation label: {argv:?}"
    );
    // Supervisor/Lead launches carry neither Peer-only label.
    assert!(
        !has_label_prefix(argv, LABEL_ASSIGNMENT),
        "supervisor launch must not carry hotpot.assignment: {argv:?}"
    );
    assert!(
        !has_label_prefix(argv, LABEL_PARENT),
        "supervisor launch must not carry hotpot.parent: {argv:?}"
    );
}

#[test]
fn lead_launch_carries_neither_peer_label() {
    let fixture = LaunchFixture::new("lead-labels");
    create_role_home(&fixture, "claude", "lead");

    let out = fixture.launch("lead", "claude", &[]);
    assert!(
        out.status.success(),
        "lead launch must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    let invocations = fixture.create_invocations();
    assert_eq!(invocations.len(), 1, "exactly one hc terminal is created");
    let argv = &invocations[0];
    assert!(
        has_label(argv, &format!("{LABEL_ROLE}lead")),
        "argv carries the lead role label: {argv:?}"
    );
    assert!(
        !has_label_prefix(argv, LABEL_ASSIGNMENT),
        "lead launch must not carry hotpot.assignment: {argv:?}"
    );
    assert!(
        !has_label_prefix(argv, LABEL_PARENT),
        "lead launch must not carry hotpot.parent: {argv:?}"
    );
}

#[test]
fn peer_launch_carries_assignment_and_parent_labels() {
    let fixture = LaunchFixture::new("peer-labels");
    create_role_home(&fixture, "pi", "peer");

    let out = fixture.launch(
        "peer",
        "pi",
        &["--assignment", "asg-42", "--parent", "term-lead-1"],
    );
    assert!(
        out.status.success(),
        "peer launch must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    let invocations = fixture.create_invocations();
    assert_eq!(invocations.len(), 1, "exactly one hc terminal is created");
    let argv = &invocations[0];
    assert!(
        has_label(argv, &format!("{LABEL_ASSIGNMENT}asg-42")),
        "peer argv carries hotpot.assignment=asg-42: {argv:?}"
    );
    assert!(
        has_label(argv, &format!("{LABEL_PARENT}term-lead-1")),
        "peer argv carries hotpot.parent=term-lead-1: {argv:?}"
    );
    assert!(
        has_label(argv, &format!("{LABEL_CORRELATION}pi-peer-asg-42")),
        "peer argv carries the assignment-scoped correlation label: {argv:?}"
    );
    assert!(
        has_label(argv, &format!("{LABEL_ROLE}peer")),
        "argv carries the peer role label: {argv:?}"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 2 — launch env/args scoped to the Role Home
// ---------------------------------------------------------------------------

/// Splits one recorded argv at the `--` separator: (hc flags, vendor command).
fn split_vendor_command(argv: &[String]) -> (Vec<String>, Vec<String>) {
    let pos = argv
        .iter()
        .position(|a| a == "--")
        .expect("argv carries the -- separator before the vendor command");
    (argv[..pos].to_vec(), argv[pos + 1..].to_vec())
}

#[test]
fn codex_launch_scopes_env_to_role_home() {
    let fixture = LaunchFixture::new("codex-env");
    let role_home = create_role_home(&fixture, "codex", "supervisor");

    let out = fixture.launch("supervisor", "codex", &[]);
    assert!(
        out.status.success(),
        "codex supervisor launch must exit 0\nstderr:\n{}",
        stderr_of(&out)
    );
    let argv = &fixture.create_invocations()[0];
    let role_home_str = role_home.to_str().unwrap();

    // The terminal env points strictly at the Role Home: HOME plus the vendor's canonical
    // home override (the same scoping `hotpot auth login` applies).
    assert!(
        has_env(argv, &format!("HOME={role_home_str}")),
        "argv carries --env HOME=<role home>: {argv:?}"
    );
    assert!(
        has_env(argv, &format!("CODEX_HOME={role_home_str}")),
        "argv carries --env CODEX_HOME=<role home>: {argv:?}"
    );

    // No operator-home path can appear anywhere in the recorded invocation.
    let operator_home = fixture.operator_home.path().to_str().unwrap();
    assert!(
        argv.iter().all(|a| !a.contains(operator_home)),
        "no argv element may reference the operator home {operator_home}: {argv:?}"
    );

    let (hc_flags, vendor) = split_vendor_command(argv);
    assert_eq!(
        vendor,
        vec![fixture.bin.path().join("codex").to_string_lossy().into_owned()],
        "the vendor command after -- is the resolved codex binary with no extra args: {hc_flags:?} | {vendor:?}"
    );

    // The terminal starts in the canonical Project root.
    let canonical_repo = fixture.repo.canonicalize().unwrap();
    assert!(
        hc_flags.windows(2).any(|w| w[0] == "--cwd" && w[1] == canonical_repo.to_string_lossy()),
        "argv carries --cwd <canonical project root>: {argv:?}"
    );
}

#[test]
fn pi_launch_pins_extension_hygiene_flags() {
    let fixture = LaunchFixture::new("pi-hygiene");
    let role_home = create_role_home(&fixture, "pi", "peer");

    let out = fixture.launch(
        "peer",
        "pi",
        &["--assignment", "asg-7", "--parent", "term-lead-1"],
    );
    assert!(
        out.status.success(),
        "pi peer launch must exit 0\nstderr:\n{}",
        stderr_of(&out)
    );
    let argv = &fixture.create_invocations()[0];
    let role_home_str = role_home.to_str().unwrap();

    // Pi scopes through HOME only (no vendor home override exists for pi).
    let env_pairs: Vec<String> = argv
        .windows(2)
        .filter(|w| w[0] == "--env")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(
        env_pairs,
        vec![format!("HOME={role_home_str}")],
        "pi carries exactly one --env pair (HOME=<role home>): {argv:?}"
    );

    let (_, vendor) = split_vendor_command(argv);
    let mut expected = vec![fixture.bin.path().join("pi").to_string_lossy().into_owned()];
    expected.extend(PI_LAUNCH_ARGS.iter().map(|s| s.to_string()));
    assert_eq!(
        vendor, expected,
        "the vendor command pins exactly the closed extension launch args"
    );
}

#[test]
fn claude_launch_pins_deny_list_flags() {
    let fixture = LaunchFixture::new("claude-hygiene");
    let role_home = create_role_home(&fixture, "claude", "lead");

    let out = fixture.launch("lead", "claude", &[]);
    assert!(
        out.status.success(),
        "claude lead launch must exit 0\nstderr:\n{}",
        stderr_of(&out)
    );
    let argv = &fixture.create_invocations()[0];
    let role_home_str = role_home.to_str().unwrap();

    assert!(
        has_env(argv, &format!("HOME={role_home_str}")),
        "argv carries --env HOME=<role home>: {argv:?}"
    );
    assert!(
        has_env(argv, &format!("CLAUDE_CONFIG_DIR={role_home_str}")),
        "argv carries --env CLAUDE_CONFIG_DIR=<role home>: {argv:?}"
    );

    let (_, vendor) = split_vendor_command(argv);
    assert_eq!(vendor[0], fixture.bin.path().join("claude").to_string_lossy().into_owned());
    for tool in CLAUDE_DENY_TOOLS {
        assert!(
            vendor.windows(2).any(|w| w[0] == "--disallow-tool" && w[1] == *tool),
            "vendor args pin --disallow-tool {tool}: {vendor:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criteria 3 & 4 — Lead discovery: reuse and duplicate halt
// ---------------------------------------------------------------------------

/// Scripts the fake `hc ls --json` answer: one entry per (address, status, role, project).
fn script_terminal_list(fixture: &LaunchFixture, entries: &[( &str, &str, &str, &str )]) {
    let list: Vec<String> = entries
        .iter()
        .map(|(address, status, role, project)| {
            format!(
                "{{\"address\":\"{address}\",\"status\":\"{status}\",\"labels\":{{
                    \"hotpot.v1\":\"true\",\"hotpot.project\":\"{project}\",
                    \"hotpot.role\":\"{role}\",\"hotpot.agent\":\"claude\"}}}}"
            )
        })
        .collect();
    fs::write(
        fixture.hc_state.path().join("list.json"),
        format!("[{}]\n", list.join(",")),
    )
    .unwrap();
}

/// AC3: with one live (or idle/resumable) Lead terminal for the project already listed, a
/// Lead launch creates no new terminal and prints a reuse notice to stdout.
#[test]
fn lead_launch_reuses_existing_live_or_resumable_lead() {
    for status in ["live", "idle", "resumable"] {
        let fixture = LaunchFixture::new(&format!("reuse-{status}"));
        create_role_home(&fixture, "claude", "lead");
        let slug = expected_slug(&fixture.repo);
        script_terminal_list(&fixture, &[("term-lead-9", status, "lead", &slug)]);

        let out = fixture.launch("lead", "claude", &[]);
        assert!(
            out.status.success(),
            "lead launch with one {status} lead must exit 0\nstdout:\n{}\nstderr:\n{}",
            stdout_of(&out),
            stderr_of(&out)
        );
        assert_eq!(
            fixture.create_invocations().len(),
            0,
            "no new terminal is created while a {status} lead exists"
        );
        let stdout = stdout_of(&out);
        assert!(
            stdout.contains("reusing"),
            "stdout prints a reuse notice: {stdout}"
        );
        assert!(
            stdout.contains("term-lead-9"),
            "reuse notice names the reused terminal: {stdout}"
        );
    }
}

/// A Lead whose terminal is neither live nor idle/resumable is not reusable: a fresh Lead
/// terminal is created for the Project.
#[test]
fn lead_launch_proceeds_when_existing_lead_is_not_resumable() {
    let fixture = LaunchFixture::new("reuse-exited");
    create_role_home(&fixture, "claude", "lead");
    let slug = expected_slug(&fixture.repo);
    script_terminal_list(&fixture, &[("term-lead-dead", "exited", "lead", &slug)]);

    let out = fixture.launch("lead", "claude", &[]);
    assert!(
        out.status.success(),
        "lead launch past a non-resumable lead must exit 0\nstderr:\n{}",
        stderr_of(&out)
    );
    assert_eq!(
        fixture.create_invocations().len(),
        1,
        "a fresh lead terminal is created when the existing one is not resumable"
    );
}

/// AC4: with two live Lead terminals for the project, the launch exits non-zero, names the
/// duplicates on stderr, and records no new terminal creation.
#[test]
fn lead_launch_halts_on_duplicate_leads() {
    let fixture = LaunchFixture::new("dup-leads");
    create_role_home(&fixture, "claude", "lead");
    let slug = expected_slug(&fixture.repo);
    script_terminal_list(
        &fixture,
        &[
            ("term-lead-a", "live", "lead", &slug),
            ("term-lead-b", "live", "lead", &slug),
        ],
    );

    let out = fixture.launch("lead", "claude", &[]);
    assert!(
        !out.status.success(),
        "duplicate leads must abort the launch with a non-zero exit"
    );
    let stderr = stderr_of(&out);
    assert!(stderr.contains("term-lead-a"), "stderr names the first duplicate: {stderr}");
    assert!(stderr.contains("term-lead-b"), "stderr names the second duplicate: {stderr}");
    assert!(
        stderr.to_lowercase().contains("duplicate") || stderr.to_lowercase().contains("escalat"),
        "stderr carries the escalation wording: {stderr}"
    );
    assert_eq!(
        fixture.create_invocations().len(),
        0,
        "no new terminal is created when duplicates are detected"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5 — invalid role/agent values — plus the injectable hc
// runner and the label-scoped discovery boundary
// ---------------------------------------------------------------------------

/// AC5: unknown role or agent values exit non-zero with a stderr message.
#[test]
fn unknown_role_or_agent_exit_non_zero_with_stderr() {
    let fixture = LaunchFixture::new("invalid-values");
    create_role_home(&fixture, "codex", "supervisor");

    let out = fixture.launch("chief", "codex", &[]);
    assert!(!out.status.success(), "unknown role must exit non-zero");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("chief") && stderr.contains("role"),
        "stderr names the invalid role: {stderr}"
    );
    assert_eq!(fixture.create_invocations().len(), 0, "nothing is launched");

    let out = fixture.launch("supervisor", "gemini", &[]);
    assert!(!out.status.success(), "unknown agent must exit non-zero");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("gemini") && stderr.contains("agent"),
        "stderr names the invalid agent: {stderr}"
    );
    assert_eq!(fixture.create_invocations().len(), 0, "nothing is launched");
}

/// The peer-only binding: `--assignment`/`--parent` belong to peer launches only, and peer
/// launches require both.
#[test]
fn assignment_and_parent_flags_are_peer_only() {
    let fixture = LaunchFixture::new("binding-rules");
    create_role_home(&fixture, "codex", "supervisor");
    create_role_home(&fixture, "codex", "peer");

    let out = fixture.launch("supervisor", "codex", &["--assignment", "asg-1", "--parent", "t1"]);
    assert!(
        !out.status.success(),
        "supervisor launch with peer binding must exit non-zero"
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("peer") && (stderr.contains("assignment") || stderr.contains("parent")),
        "stderr explains the peer-only binding: {stderr}"
    );

    let out = fixture.launch("peer", "codex", &[]);
    assert!(
        !out.status.success(),
        "peer launch without --assignment/--parent must exit non-zero"
    );
    assert!(
        stderr_of(&out).contains("peer"),
        "stderr explains the missing peer binding: {}",
        stderr_of(&out)
    );
    assert_eq!(fixture.create_invocations().len(), 0, "nothing is launched");
}

/// The `hc` binary is injectable: `HOTPOT_HC` names it even when no `hc` is on `PATH`
/// (ticket 06 context: `src/hc.rs` must not hard-code `Command::new("hc")`).
#[test]
fn hc_runner_is_injectable_via_hotpot_hc_env() {
    let fixture = LaunchFixture::new("hc-env");
    create_role_home(&fixture, "codex", "supervisor");

    // A vendor-only PATH (no hc anywhere on it); the fake hc is reached via HOTPOT_HC.
    let vendor_only_bin = TempDir::new("seat-vendor-only");
    for vendor in ["codex", "claude", "pi"] {
        write_fake_vendor(vendor_only_bin.path(), vendor);
    }
    let out = Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args([
            "seat",
            "launch",
            "supervisor",
            "--agent",
            "codex",
            "--project",
        ])
        .arg(&fixture.repo)
        .env("HOTPOT_HOME", fixture.home.path())
        .env("HOME", fixture.operator_home.path())
        .env("HOTPOT_FAKE_HC_STATE", fixture.hc_state.path())
        .env("HOTPOT_HC", fixture.bin.path().join("hc"))
        .env("PATH", sandbox_path(vendor_only_bin.path()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "launch succeeds with hc injected via HOTPOT_HC\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert_eq!(
        fixture.create_invocations().len(),
        1,
        "the injected hc runner created the terminal"
    );
}

/// Lead discovery is scoped by labels: a live Lead of another project (or a hotpot
/// Supervisor terminal) never blocks or reuses this project's Lead launch.
#[test]
fn lead_discovery_is_scoped_to_the_project_labels() {
    let fixture = LaunchFixture::new("scope-labels");
    create_role_home(&fixture, "claude", "lead");
    let slug = expected_slug(&fixture.repo);
    script_terminal_list(
        &fixture,
        &[
            ("term-lead-other", "live", "lead", "other-project-abcdef"),
            ("term-sup", "live", "supervisor", &slug),
        ],
    );

    let out = fixture.launch("lead", "claude", &[]);
    assert!(
        out.status.success(),
        "another project's lead and this project's supervisor must not block the launch\nstderr:\n{}",
        stderr_of(&out)
    );
    assert_eq!(
        fixture.create_invocations().len(),
        1,
        "this project's lead is launched fresh; no reuse from mismatched labels"
    );
}

/// AC (ticket 11): a Lead terminal archived by a completed quiet-point
/// replacement is never a reuse candidate — the successor, not the archived
/// terminal, is the Project's Lead, so a Lead launch creates a fresh terminal.
#[test]
fn lead_launch_never_reuses_an_archived_lead_terminal() {
    let fixture = LaunchFixture::new("reuse-archived");
    create_role_home(&fixture, "claude", "lead");
    let slug = expected_slug(&fixture.repo);
    script_terminal_list(&fixture, &[("term-old-1", "live", "lead", &slug)]);
    // The runtime ledger records the quiet-point replacement that archived it.
    let ledger_dir = fixture
        .home
        .path()
        .join("runtime")
        .join("v1")
        .join(&slug);
    fs::create_dir_all(&ledger_dir).unwrap();
    fs::write(
        ledger_dir.join("ledger.jsonl"),
        "{\"type\":\"lead_archived\",\"replacement\":\"lrep_001\",\"terminal\":\"term-old-1\",\"seq\":1,\"ts\":0}\n",
    )
    .unwrap();

    let out = fixture.launch("lead", "claude", &[]);
    assert!(
        out.status.success(),
        "lead launch past an archived lead must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert_eq!(
        fixture.create_invocations().len(),
        1,
        "a fresh successor terminal is created; the archived terminal is not reused"
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("launched"),
        "stdout reports a launch, not a reuse: {stdout}"
    );
}

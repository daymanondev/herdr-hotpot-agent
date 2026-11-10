//! Seam tests for the real `hotpot status` / `hotpot inspect` data layer (ticket 12
//! of the Hotpot Foundation spec): live projections of the runtime ledger, `hc`
//! terminal labels, and parsed `.jsonl` transcripts — with segmented context,
//! attention items, and uncollapsed thinking — replacing every mocked fixture.
//!
//! Seam (ticket 12 "Testing seam"): the `hotpot status` / `hotpot inspect` CLI from
//! `cargo test` against a temp `HOTPOT_HOME` seeded with fixture ledger events and
//! transcript files plus the fake `hc` runner; assert JSON stdout shapes and exit
//! codes. Zero network; no private-internal assertions.

mod common;

use common::{TempDir, expected_slug, init_git_repo, seed_ledger_event_at};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

/// A fixed "now" for the injectable clock: fixture timestamps are relative to it,
/// so the 5-minute stalled-permission mark is exact.
const NOW: u64 = 1_700_000_000;

/// Runs the `hotpot` CLI with the injectable environment seams (`HOTPOT_NOW`,
/// `HOTPOT_HC` + fake state) and returns the raw output.
fn run_hotpot(home: &Path, args: &[&str], envs: &[(&str, String)]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hotpot"));
    cmd.args(args)
        .env("HOTPOT_HOME", home)
        .env("HOTPOT_NOW", NOW.to_string());
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("spawn hotpot CLI")
}

/// Parses stdout of a successful `hotpot status` run, asserting exit 0.
fn status_json(out: &std::process::Output) -> Value {
    assert!(
        out.status.success(),
        "hotpot status exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "status stdout is JSON: {e}\nstdout:\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The seat entry of a status snapshot with the given id.
fn seat_of<'a>(status: &'a Value, id: &str) -> &'a Value {
    status["seats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == json!(id))
        .unwrap_or_else(|| panic!("seat `{id}` present: {}", status["seats"]))
}

/// Writes the fake `hc` runner (ticket 06 pattern): `hc ls --json` prints the
/// scripted `<state>/list.json`; `hc run ...` records its argv.
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

/// An executable no-op shim (fake `hc` via `HOTPOT_HC`, fake vendor on `PATH`).
fn write_shim(dir: &Path, name: &str) {
    let shim = dir.join(name);
    fs::write(&shim, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Sets a file's mtime so newest-transcript-wins is deterministic.
fn touch_old(path: &Path, ago: Duration) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_times(
        fs::FileTimes::new().set_modified(SystemTime::now() - ago),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1 (+3, +4) — status projects the live ledger & terminals
// ---------------------------------------------------------------------------

#[test]
fn status_projects_seats_assignments_mailbox_and_attention_from_the_fixture() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let repo_path = repo.path().to_string_lossy().into_owned();
    let slug = expected_slug(repo.path());
    let bin = TempDir::new("hcbin");
    let hc_state = TempDir::new("hcstate");
    write_fake_hc(bin.path());
    let envs = [
        ("HOTPOT_HC", bin.path().join("hc").to_string_lossy().into_owned()),
        (
            "HOTPOT_FAKE_HC_STATE",
            hc_state.path().to_string_lossy().into_owned(),
        ),
    ];

    // The fake `hc` terminal listing: supervisor, lead, and the assignment's peer,
    // all carrying the canonical label set for this Project.
    fs::write(
        hc_state.path().join("list.json"),
        json!([
            {
                "address": "term-sup-7",
                "status": "live",
                "labels": {
                    "hotpot.v1": "true",
                    "hotpot.project": slug,
                    "hotpot.role": "supervisor",
                    "hotpot.agent": "claude",
                    "hotpot.correlation": "claude-supervisor"
                }
            },
            {
                "address": "term-lead-8",
                "status": "idle",
                "labels": {
                    "hotpot.v1": "true",
                    "hotpot.project": slug,
                    "hotpot.role": "lead",
                    "hotpot.agent": "codex",
                    "hotpot.correlation": "codex-lead"
                }
            },
            {
                "address": "term-peer-9",
                "status": "live",
                "labels": {
                    "hotpot.v1": "true",
                    "hotpot.project": slug,
                    "hotpot.role": "peer",
                    "hotpot.agent": "pi",
                    "hotpot.correlation": "pi-peer-asg_001",
                    "hotpot.assignment": "asg_001",
                    "hotpot.parent": "term-lead-8"
                }
            }
        ])
        .to_string()
            + "\n",
    )
    .unwrap();

    // Role profile defaults in Hotpot Home (the read side the pill pickers of
    // tickets 13/14 will update): the Lead's model/thinking, and a peer default
    // the dispatch-recorded model and the assignment's thinking must beat.
    fs::write(
        home.path().join("profiles.json"),
        json!({
            "lead": {"agent": "codex", "model": "gpt-5.1-codex", "thinking": "high"},
            "peer": {"agent": "pi", "model": "default-peer-model", "thinking": "low"}
        })
        .to_string(),
    )
    .unwrap();

    // The fixture ledger: launched seats, one assignment lifecycle, a mailbox
    // directive, segmented context usage, and one stalled permission.
    let seed = |event: Value, ts: u64| seed_ledger_event_at(home.path(), repo.path(), event, ts);
    seed(
        json!({"type": "seat_launched", "seat": "claude-supervisor", "role": "supervisor", "agent": "claude"}),
        NOW - 5000,
    );
    seed(
        json!({"type": "seat_launched", "seat": "codex-lead", "role": "lead", "agent": "codex"}),
        NOW - 5000,
    );
    seed(
        json!({
            "type": "assignment_created",
            "assignment": "asg_001",
            "outcome": "Ship the real status data layer",
            "disposition": "Engineer",
            "baseCommit": head,
            "gate": "./scripts/check.sh",
            "thinking": "high",
            "thinkingReason": "live projection, no canned values"
        }),
        NOW - 4000,
    );
    seed(
        json!({
            "type": "assignment_dispatched",
            "assignment": "asg_001",
            "correlation": "pi-peer-asg_001",
            "workspace": repo_path,
            "agent": "pi",
            "model": "zai-glm-chatter"
        }),
        NOW - 3000,
    );
    seed(
        json!({
            "type": "message_to_lead",
            "from": "supervisor",
            "message": "Human directive: land the data layer this week"
        }),
        NOW - 2000,
    );
    seed(
        json!({
            "type": "assignment_handed_back",
            "assignment": "asg_001",
            "correlation": "pi-peer-asg_001",
            "summary": "Implemented and gate green",
            "candidate": head
        }),
        NOW - 100,
    );
    seed(
        json!({"type": "gate_completed", "assignment": "asg_001", "verdict": "green"}),
        NOW - 90,
    );
    // Segmented context: the Lead below the rotation mark, the peer past it.
    seed(
        json!({"type": "context_usage", "seat": "codex-lead", "cacheRead": 100000, "freshInput": 40000, "output": 5000}),
        NOW - 50,
    );
    seed(
        json!({"type": "context_usage", "seat": "pi-peer-asg_001", "cacheRead": 120000, "freshInput": 25000, "output": 8000}),
        NOW - 40,
    );
    // One attention condition: a permission stalled past the 5-minute mark.
    seed(
        json!({
            "type": "permission_requested",
            "permission": "perm_9",
            "assignment": "asg_001",
            "detail": "Bash(rm -rf) approval"
        }),
        NOW - 400,
    );

    let out = run_hotpot(
        home.path(),
        &["status", "--workspace", &repo_path, "--state-dir", home.path().to_str().unwrap()],
        &envs,
    );
    let status = status_json(&out);

    assert_eq!(status["project"], json!(slug), "project slug is canonical");
    assert_eq!(status["workspace"], json!(repo_path));
    // The watch target the mods subscribe to with `watchFolder`: the runtime
    // root under Hotpot Home, now that seeded runtime state exists.
    assert_eq!(
        status["runtimeDir"],
        json!(home.path().join("runtime").join("v1").to_string_lossy().into_owned()),
        "runtimeDir names <HOTPOT_HOME>/runtime/v1 for the mods' watchFolder"
    );
    assert_eq!(
        status["seats"].as_array().unwrap().len(),
        3,
        "supervisor, lead, and the dispatched peer project as seats: {}",
        status["seats"]
    );

    // Supervisor: from the hc label join; no usage events -> zeroed segments, no tick.
    let sup = seat_of(&status, "claude-supervisor");
    assert_eq!(sup["role"], json!("supervisor"));
    assert_eq!(sup["agent"], json!("claude"));
    assert_eq!(sup["status"], json!("live"));
    assert_eq!(sup["terminal"], json!("term-sup-7"));
    assert_eq!(sup["model"], Value::Null, "no profile default recorded");
    assert_eq!(
        sup["context"],
        json!({"cacheRead": 0, "freshInput": 0, "output": 0, "total": 0, "rotationTick": false})
    );

    // Lead: model/thinking from the role profile defaults, segmented context from
    // its latest usage event (100k + 40k + 5k = 145k < 150k -> no rotation tick).
    let lead = seat_of(&status, "codex-lead");
    assert_eq!(lead["agent"], json!("codex"));
    assert_eq!(lead["status"], json!("idle"));
    assert_eq!(lead["terminal"], json!("term-lead-8"));
    assert_eq!(lead["model"], json!("gpt-5.1-codex"));
    assert_eq!(lead["thinking"], json!("high"));
    assert_eq!(
        lead["context"],
        json!({"cacheRead": 100000, "freshInput": 40000, "output": 5000, "total": 145000, "rotationTick": false})
    );

    // Peer: dispatch-recorded model beats the profile default, the assignment's
    // thinking effort beats the profile default, and its 153k total ticks rotation.
    let peer = seat_of(&status, "pi-peer-asg_001");
    assert_eq!(peer["role"], json!("peer"));
    assert_eq!(peer["assignment"], json!("asg_001"));
    assert_eq!(peer["model"], json!("zai-glm-chatter"));
    assert_eq!(peer["thinking"], json!("high"));
    assert_eq!(peer["terminal"], json!("term-peer-9"));
    assert_eq!(peer["context"]["total"], json!(153000));
    assert_eq!(peer["context"]["rotationTick"], json!(true));

    // The assignments master list mirrors the fixture events exactly.
    let tasks = status["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1, "one fixture assignment: {tasks:?}");
    assert_eq!(tasks[0]["id"], json!("asg_001"));
    assert_eq!(tasks[0]["state"], json!("HandedBack"));
    assert_eq!(tasks[0]["disposition"], json!("Engineer"));
    assert_eq!(tasks[0]["verdict"], json!("green"));
    assert_eq!(tasks[0]["attempt"], json!(1));
    assert_eq!(tasks[0]["summary"], json!("Ship the real status data layer"));
    assert_eq!(tasks[0]["correlation"], json!("pi-peer-asg_001"));
    assert_eq!(tasks[0]["terminal"], json!("term-peer-9"));

    // The mailbox mirrors the fixture mailbox events, in event order.
    let mailbox = status["mailbox"].as_array().unwrap();
    assert_eq!(
        mailbox,
        &vec![
            json!({
                "from": "supervisor",
                "to": "lead",
                "kind": "directive",
                "summary": "Human directive: land the data layer this week"
            }),
            json!({
                "from": "pi-peer-asg_001",
                "to": "lead",
                "kind": "handback",
                "summary": "Implemented and gate green"
            }),
        ],
        "mailbox projects the fixture message_to_lead and handback events"
    );

    // Attention projection: the stalled permission surfaces as a letter keyed by
    // its stable att_<id> key.
    let attention = status["attention"].as_array().unwrap();
    assert_eq!(attention.len(), 1, "one due letter: {attention:?}");
    assert_eq!(attention[0]["key"], json!("att_stalled_permission_perm_9"));
    assert_eq!(attention[0]["kind"], json!("stalled-permission"));

    // Changing the fixture changes the output: a second assignment appears.
    seed(
        json!({
            "type": "assignment_created",
            "assignment": "asg_002",
            "outcome": "A second fixture outcome that must surface verbatim",
            "disposition": "Scout",
            "baseCommit": head,
            "gate": "./scripts/check.sh"
        }),
        NOW - 10,
    );
    let out = run_hotpot(
        home.path(),
        &["status", "--workspace", &repo_path, "--state-dir", home.path().to_str().unwrap()],
        &envs,
    );
    let status = status_json(&out);
    let tasks = status["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2, "the seeded second assignment projects: {tasks:?}");
    assert_eq!(tasks[1]["id"], json!("asg_002"));
    assert_eq!(tasks[1]["state"], json!("Draft"));
    assert_eq!(
        tasks[1]["summary"],
        json!("A second fixture outcome that must surface verbatim")
    );
}

// ---------------------------------------------------------------------------
// CLI dispatch — unknown subcommands fail closed; bare `hotpot` means status
// ---------------------------------------------------------------------------

#[test]
fn unknown_subcommand_fails_closed_while_bare_hotpot_answers_status() {
    let home = TempDir::new("home");
    let workspace = TempDir::new("ws");

    // A typo must not silently answer with the status JSON and exit 0.
    let out = run_hotpot(home.path(), &["setu"], &[]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an unknown subcommand exits 2, not 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown command") && stderr.contains("setu") && stderr.contains("--help"),
        "stderr names the unknown command and points at help: {stderr}"
    );
    assert!(
        out.stdout.is_empty(),
        "no status JSON leaks to stdout for an unknown subcommand"
    );

    // A bare `hotpot` (no arguments) keeps its documented meaning: `status`.
    let out = Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .env("HOTPOT_HOME", home.path())
        .env("HOTPOT_NOW", NOW.to_string())
        .env("HOTPOT_HC", home.path().join("missing-hc").to_string_lossy().into_owned())
        .current_dir(workspace.path())
        .output()
        .expect("spawn bare hotpot");
    assert!(
        out.status.success(),
        "bare hotpot exits 0 as status: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("bare hotpot stdout is status JSON: {e}"));
    assert_eq!(status["seats"], json!([]), "bare hotpot serves the status projection");
}

// ---------------------------------------------------------------------------
// Acceptance criterion 2 — empty state stays empty (no mocked values)
// ---------------------------------------------------------------------------

#[test]
fn status_on_an_empty_home_exits_zero_with_empty_arrays() {
    let home = TempDir::new("home");
    let workspace = TempDir::new("ws");
    // No hc anywhere: the injected binary path does not exist.
    let envs = [("HOTPOT_HC", home.path().join("missing-hc").to_string_lossy().into_owned())];

    // HOTPOT_HOME (not --state-dir) is honored: the mods invoke a bare
    // `hotpot status --workspace <folder>`.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hotpot"));
    cmd.args(["status", "--workspace", workspace.path().to_str().unwrap()])
        .env("HOTPOT_HOME", home.path())
        .env("HOTPOT_NOW", NOW.to_string());
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("spawn hotpot status");
    let status = status_json(&out);

    assert_eq!(status["seats"], json!([]));
    assert_eq!(status["tasks"], json!([]));
    assert_eq!(status["mailbox"], json!([]));
    assert_eq!(status["attention"], json!([]));
    // No runtime state on disk yet: no watch target either — the mods keep
    // their timer fallback instead of subscribing to a missing directory.
    assert_eq!(status["runtimeDir"], Value::Null, "runtimeDir stays null on an empty home");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // `claude-opus-4-6` left this list with ticket nui/01: it is now a
    // legitimate built-in profile default served in `profiles` on a fresh
    // Project, not a leaked mock.
    for mock in [
        "seat-supervisor",
        "seat-lead",
        "T-01",
        "peer-T-01",
        "gpt-5.4",
        "Conversational Owner delegate",
    ] {
        assert!(!stdout.contains(mock), "mocked value `{mock}` leaked into empty status");
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5 — inspect parses real Claude/Codex transcripts
// ---------------------------------------------------------------------------

/// The Lead's uncollapsed thinking body: long enough (and awkwardly wrapped
/// enough) that any truncation or collapsing loses content.
const LONG_THINKING: &str = "Premise check before wiring the parser. \
The ticket names parse_session_jsonl as the reusable piece, so inspect must not re-derive \
its own event shapes: the .jsonl stream is the contract. I will keep every line of this \
block verbatim, including this trailing sentence, because the Session Inspector renders \
thinking expanded by default and the whole point of this ticket is that nothing is summarized away.";

/// Writes a Claude-style transcript under the lead Role Home's `projects/` root.
fn write_claude_transcript(home: &Path) {
    let dir = home.join("roles").join("claude").join("lead").join("projects");
    fs::create_dir_all(&dir).unwrap();
    let stale = dir.join("sess-stale.jsonl");
    let live = dir.join("sess-live.jsonl");
    fs::write(
        &stale,
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"stale session\"}]}}\n",
    )
    .unwrap();
    let lines = vec![
        json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "text", "text": "Land ticket 12 with parse_session_jsonl wired to inspect"}
        ]}}),
        json!({"type": "assistant", "message": {"role": "assistant", "usage": {
            "input_tokens": 12000, "cache_read_input_tokens": 30000, "output_tokens": 900
        }, "content": [
            {"type": "thinking", "thinking": LONG_THINKING, "duration_ms": 3400},
            {"type": "tool_use", "id": "call-1", "name": "Bash", "input": {"command": "cargo test"}},
            {"type": "tool_use", "id": "call-2", "name": "Read", "input": {"file_path": "src/main.rs"}},
            {"type": "text", "text": "Wiring the parser into inspect now."}
        ]}}),
        json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "call-1", "content": "ok. 12 passed"}
        ]}}),
    ];
    let mut file = fs::File::create(&live).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
    // The stale session is older, so the live one must win.
    touch_old(&stale, Duration::from_secs(3600));
    touch_old(&live, Duration::from_secs(60));
}

/// Writes a Codex-style rollout under the supervisor Role Home's `sessions/` root.
fn write_codex_transcript(home: &Path) {
    let dir = home
        .join("roles")
        .join("codex")
        .join("supervisor")
        .join("sessions")
        .join("2025")
        .join("11")
        .join("10");
    fs::create_dir_all(&dir).unwrap();
    let lines = vec![
        json!({"timestamp": "2025-11-10T09:00:00Z", "type": "event_msg", "payload": {"type": "task_started"}}),
        json!({"timestamp": "2025-11-10T09:00:01Z", "type": "response_item", "payload": {
            "type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "Observe the room and surface attention letters"}
            ]
        }}),
        json!({"timestamp": "2025-11-10T09:00:02Z", "type": "response_item", "payload": {
            "type": "reasoning", "summary": [
                {"type": "summary_text", "text": "Checking the ledger before drafting the first observation batch."}
            ]
        }}),
        json!({"timestamp": "2025-11-10T09:00:03Z", "type": "response_item", "payload": {
            "type": "function_call", "name": "room_status", "arguments": "{}", "call_id": "fc-1"
        }}),
        json!({"timestamp": "2025-11-10T09:00:04Z", "type": "response_item", "payload": {
            "type": "function_call_output", "call_id": "fc-1", "output": "{\"allQuiet\":true}"
        }}),
        json!({"timestamp": "2025-11-10T09:00:05Z", "type": "event_msg", "payload": {
            "type": "task_complete", "last_agent_message": "All quiet; no letters due."
        }}),
    ];
    let mut file = fs::File::create(dir.join("rollout-fixture.jsonl")).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

#[test]
fn inspect_returns_uncollapsed_items_from_real_transcripts() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    let repo_path = repo.path().to_string_lossy().into_owned();

    let seed = |event: Value, ts: u64| seed_ledger_event_at(home.path(), repo.path(), event, ts);
    seed(
        json!({"type": "seat_launched", "seat": "claude-lead", "role": "lead", "agent": "claude"}),
        NOW - 1000,
    );
    seed(
        json!({"type": "seat_launched", "seat": "codex-supervisor", "role": "supervisor", "agent": "codex"}),
        NOW - 1000,
    );
    // The supervisor reports usage only through the ledger (its rollout fixture
    // carries no usage lines): inspect must fall back to the seat's usage event.
    seed(
        json!({"type": "context_usage", "seat": "codex-supervisor", "cacheRead": 1000, "freshInput": 200, "output": 50}),
        NOW - 30,
    );
    write_claude_transcript(home.path());
    write_codex_transcript(home.path());

    // --- Claude-style transcript -------------------------------------------
    let out = run_hotpot(
        home.path(),
        &[
            "inspect",
            "--agent",
            "claude-lead",
            "--workspace",
            &repo_path,
            "--state-dir",
            home.path().to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "inspect exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let inspection: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("inspect stdout is JSON: {e}\n{}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(inspection["agent_id"], json!("claude-lead"));
    assert_eq!(inspection["role"], json!("lead"));
    assert!(
        inspection["transcript"]
            .as_str()
            .is_some_and(|p| p.ends_with("sess-live.jsonl")),
        "the newest transcript wins: {}",
        inspection["transcript"]
    );
    let items = inspection["items"].as_array().unwrap();
    let kinds: Vec<&str> = items.iter().filter_map(|i| i["kind"].as_str()).collect();
    assert_eq!(
        kinds,
        vec!["message", "thinking", "tool_call", "tool_call", "message"],
        "parsed kinds in order: {items:?}"
    );
    // Thinking is preserved in full — byte-for-byte, never collapsed.
    assert_eq!(items[1]["body"], json!(LONG_THINKING));
    assert_eq!(items[1]["duration_ms"], json!(3400));
    // Tool calls carry a status: answered by a tool_result -> done, still
    // unanswered -> running.
    assert_eq!(items[2]["title"], json!("Bash"));
    assert_eq!(items[2]["status"], json!("done"));
    assert_eq!(items[3]["title"], json!("Read"));
    assert_eq!(items[3]["status"], json!("running"));
    // Per-seat context segmentation from the transcript's own usage line.
    assert_eq!(
        inspection["context"],
        json!({"cacheRead": 30000, "freshInput": 12000, "output": 900, "total": 42900, "rotationTick": false})
    );

    // --- Codex-style transcript --------------------------------------------
    let out = run_hotpot(
        home.path(),
        &[
            "inspect",
            "--agent",
            "codex-supervisor",
            "--workspace",
            &repo_path,
            "--state-dir",
            home.path().to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "inspect exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let inspection: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(inspection["role"], json!("supervisor"));
    let items = inspection["items"].as_array().unwrap();
    let kinds: Vec<&str> = items.iter().filter_map(|i| i["kind"].as_str()).collect();
    assert_eq!(
        kinds,
        vec!["notice", "message", "thinking", "tool_call", "notice"],
        "notice kinds surface from rollout task events: {items:?}"
    );
    // Reasoning summary text stays uncollapsed; the completed call is done.
    assert_eq!(
        items[2]["body"],
        json!("Checking the ledger before drafting the first observation batch.")
    );
    assert_eq!(items[3]["title"], json!("room_status"));
    assert_eq!(items[3]["status"], json!("done"));
    assert_eq!(items[4]["title"], json!("Task complete"));
    assert_eq!(items[4]["body"], json!("All quiet; no letters due."));
    // No usage lines in the rollout: the seat's ledger usage event segments it.
    assert_eq!(
        inspection["context"],
        json!({"cacheRead": 1000, "freshInput": 200, "output": 50, "total": 1250, "rotationTick": false})
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 6 — a seat without a transcript stays quiet, exit 0
// ---------------------------------------------------------------------------

#[test]
fn inspect_for_a_seat_without_a_transcript_exits_zero_with_a_note() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({"type": "seat_launched", "seat": "pi-peer-asg_777", "role": "peer", "agent": "pi", "assignment": "asg_777"}),
        NOW - 10,
    );

    let out = run_hotpot(
        home.path(),
        &[
            "inspect",
            "--agent",
            "pi-peer-asg_777",
            "--workspace",
            repo.path().to_str().unwrap(),
            "--state-dir",
            home.path().to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "inspect exits 0 for a transcript-less seat: {}",
        out.status
    );
    let inspection: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(inspection["items"], json!([]));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no transcript"),
        "a stderr note names the missing transcript: {stderr}"
    );

    // An unknown seat (no record anywhere) is the same quiet answer.
    let out = run_hotpot(
        home.path(),
        &[
            "inspect",
            "--agent",
            "nobody-here",
            "--workspace",
            repo.path().to_str().unwrap(),
            "--state-dir",
            home.path().to_str().unwrap(),
        ],
        &[],
    );
    assert!(out.status.success());
    let inspection: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(inspection["items"], json!([]));
}

// ---------------------------------------------------------------------------
// Seat launches are recorded in the ledger, so status survives without `hc`
// ---------------------------------------------------------------------------

#[test]
fn seat_launch_records_the_seat_and_status_projects_it_without_hc() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("operator");
    let bin = TempDir::new("bin");
    let hc_state = TempDir::new("hcstate");
    let (repo, _head) = init_git_repo();
    write_fake_hc(bin.path());
    write_shim(bin.path(), "codex");
    // The lead-launch discovery listing: no existing terminals.
    fs::write(hc_state.path().join("list.json"), "[]\n").unwrap();
    // The lead Role Home must exist (launch refuses without `hotpot setup --apply`).
    fs::create_dir_all(home.path().join("roles").join("codex").join("lead")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args([
            "seat",
            "launch",
            "lead",
            "--agent",
            "codex",
            "--project",
            repo.path().to_str().unwrap(),
        ])
        .env("HOTPOT_HOME", home.path())
        .env("HOME", fake_home.path())
        .env("HOTPOT_HC", bin.path().join("hc"))
        .env("HOTPOT_FAKE_HC_STATE", hc_state.path())
        // Prepend the fake bin dir so the vendor shim resolves first while the
        // fake `hc` script keeps its `cat`.
        .env(
            "PATH",
            format!("{}:{}", bin.path().display(), std::env::var("PATH").unwrap_or_default()),
        )
        .output()
        .expect("spawn hotpot seat launch");
    assert!(
        out.status.success(),
        "seat launch succeeds: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The launch is durable in the project ledger.
    let slug = expected_slug(repo.path());
    let ledger = home
        .path()
        .join("runtime")
        .join("v1")
        .join(&slug)
        .join("ledger.jsonl");
    let events: Vec<Value> = fs::read_to_string(&ledger)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line parses"))
        .collect();
    let launched: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == json!("seat_launched"))
        .collect();
    assert_eq!(launched.len(), 1, "one seat_launched event: {events:?}");
    assert_eq!(launched[0]["seat"], json!("codex-lead"));
    assert_eq!(launched[0]["role"], json!("lead"));
    assert_eq!(launched[0]["agent"], json!("codex"));

    // With hc unreachable, status still projects the seat from the ledger.
    let out = run_hotpot(
        home.path(),
        &[
            "status",
            "--workspace",
            repo.path().to_str().unwrap(),
            "--state-dir",
            home.path().to_str().unwrap(),
        ],
        &[("HOTPOT_HC", home.path().join("missing-hc").to_string_lossy().into_owned())],
    );
    let status = status_json(&out);
    let lead = seat_of(&status, "codex-lead");
    assert_eq!(lead["role"], json!("lead"));
    assert_eq!(lead["agent"], json!("codex"));
    assert_eq!(lead["status"], json!("offline"));
    assert_eq!(lead["terminal"], Value::Null);
}

// ---------------------------------------------------------------------------
// Ticket nui/01 — backward-compatible status metadata: `provisioned`,
// `hasWorkspaceProtocol`, and the resolved `profiles` defaults
// ---------------------------------------------------------------------------

/// Runs `hotpot status` against `home`/`repo` and returns the parsed snapshot.
fn status_for(home: &Path, repo: &Path) -> Value {
    let out = run_hotpot(
        home,
        &[
            "status",
            "--workspace",
            &repo.to_string_lossy(),
            "--state-dir",
            &home.to_string_lossy(),
        ],
        &[("HOTPOT_HC", home.join("missing-hc").to_string_lossy().into_owned())],
    );
    status_json(&out)
}

#[test]
fn status_serves_markers_and_builtin_profile_defaults_on_a_fresh_project() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();

    // Fresh Project: neither marker exists, yet every role still resolves to a
    // concrete default profile (the panel's always-present Seat Slots).
    let status = status_for(home.path(), repo.path());
    assert_eq!(
        status["provisioned"],
        json!(false),
        "no `hotpot.json` marker in Hotpot Home yet"
    );
    assert_eq!(
        status["hasWorkspaceProtocol"],
        json!(false),
        "no WORKSPACE_PROTOCOL.md at the Project root yet"
    );
    assert_eq!(
        status["profiles"],
        json!({
            "supervisor": {"agent": "claude", "model": "claude-opus-4-6", "thinking": "high"},
            "lead": {"agent": "codex", "model": "gpt-5.1-codex", "thinking": "high"},
            "peer": {"agent": "codex", "model": "gpt-5.1-codex", "thinking": "medium"},
        }),
        "the three roles resolve to the built-in defaults"
    );
    // Backward compatibility: the recorded surfaces stay empty arrays.
    for field in ["seats", "tasks", "attention", "mailbox"] {
        assert_eq!(status[field], json!([]), "`{field}` stays an empty array");
    }

    // Each marker flips only its own flag: `hotpot.json` in Hotpot Home,
    // `WORKSPACE_PROTOCOL.md` at the Project root.
    fs::write(home.path().join("hotpot.json"), "{}\n").unwrap();
    let status = status_for(home.path(), repo.path());
    assert_eq!(status["provisioned"], json!(true));
    assert_eq!(status["hasWorkspaceProtocol"], json!(false));

    fs::write(repo.path().join("WORKSPACE_PROTOCOL.md"), "# Workspace Protocol\n").unwrap();
    let status = status_for(home.path(), repo.path());
    assert_eq!(status["provisioned"], json!(true));
    assert_eq!(status["hasWorkspaceProtocol"], json!(true));
}

#[test]
fn status_resolves_profiles_json_entries_per_field_over_the_builtins() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();

    // A hand-edited partial entry: only the Lead's model is set, so its agent
    // and thinking must still resolve from the built-ins.
    fs::write(
        home.path().join("profiles.json"),
        json!({"lead": {"model": "x"}}).to_string(),
    )
    .unwrap();
    let status = status_for(home.path(), repo.path());
    assert_eq!(
        status["profiles"]["lead"],
        json!({"agent": "codex", "model": "x", "thinking": "high"}),
        "the partial lead entry overrides only its model field"
    );
    assert_eq!(
        status["profiles"]["supervisor"],
        json!({"agent": "claude", "model": "claude-opus-4-6", "thinking": "high"}),
        "untouched roles keep the built-in defaults"
    );
    assert_eq!(
        status["profiles"]["peer"],
        json!({"agent": "codex", "model": "gpt-5.1-codex", "thinking": "medium"}),
    );

    // The panel's write path — `hotpot profile set` (ticket 13) — surfaces the
    // same way: a full supervisor entry lands, the lead override survives.
    // (The subcommand resolves Hotpot Home from HOTPOT_HOME, which `run_hotpot`
    // always sets.)
    let out = run_hotpot(
        home.path(),
        &[
            "profile",
            "set",
            "claude",
            "supervisor",
            "claude-sonnet-4-5",
            "--thinking",
            "low",
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "profile set exits 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = status_for(home.path(), repo.path());
    assert_eq!(
        status["profiles"]["supervisor"],
        json!({"agent": "claude", "model": "claude-sonnet-4-5", "thinking": "low"}),
        "the profile set write resolves as the supervisor defaults"
    );
    assert_eq!(status["profiles"]["lead"]["model"], json!("x"));
    assert_eq!(status["profiles"]["peer"]["model"], json!("gpt-5.1-codex"));
}

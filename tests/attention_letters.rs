//! Seam tests for the Supervisor attention surface (ticket 11 of the Hotpot
//! Foundation spec): batched, rate-limited Attention Letters over hand-written
//! fixture ledger lines with an injectable clock (`HOTPOT_NOW`), `attention_feedback`
//! suppression, `message_lead` delivery through the injectable `hc` runner, and
//! quiet-point Lead replacement via the 06 seat launcher.
//!
//! Seam (ticket 11 "Testing seam"): Supervisor MCP stdio sessions from `cargo test`
//! against a temp `HOTPOT_HOME` and a temp real git repository; assert
//! `room_status`/`runtime_findings` JSON letter entries, feedback effects, and
//! recorded fake `hc` invocations. Zero network; no private-internal assertions.

mod common;

use common::{McpProcess, TempDir, init_git_repo, seed_ledger_event_at};
use serde_json::{Value, json};
use std::path::Path;

/// Creates and dispatches a fixture assignment through a real Lead session so
/// the projection replays it; returns the assignment id.
fn dispatched_assignment(home: &Path, repo: &Path, head: &str) -> String {
    let mut lead = McpProcess::spawn("lead", repo, home);
    let created = lead.tools_call(
        json!("create"),
        "assignment_create",
        json!({
            "outcome": "Ship the attention surface",
            "disposition": "Engineer",
            "baseCommit": head,
            "writeScope": ["src/**"],
            "gate": "./scripts/check.sh",
        }),
    );
    assert!(
        created.get("error").is_none(),
        "fixture assignment_create must succeed: {created}"
    );
    let id = created["result"]["id"].as_str().unwrap().to_string();
    let dispatched = lead.tools_call(
        json!("dispatch"),
        "assignment_dispatch",
        json!({"assignment": id, "correlation": "corr-fixture"}),
    );
    assert!(
        dispatched.get("error").is_none(),
        "fixture dispatch must succeed: {dispatched}"
    );
    id
}

/// A fixed "now" for the injectable clock: every fixture timestamp is expressed
/// relative to it, so the five-minute / ten-minute marks are exact.
const NOW: u64 = 1_700_000_000;

/// Spawns a Supervisor MCP session whose clock is pinned to `NOW`.
fn supervisor(home: &Path, repo: &Path) -> McpProcess {
    McpProcess::spawn_with_env(
        "supervisor",
        repo,
        home,
        None,
        &[("HOTPOT_NOW", &NOW.to_string())],
    )
}

/// The attention letters array of a `room_status` result (asserting the shape).
fn letters_of(room_status: &Value) -> Vec<Value> {
    room_status["result"]["attention"]["letters"]
        .as_array()
        .unwrap_or_else(|| panic!("room_status carries attention.letters: {room_status}"))
        .clone()
}

/// The letter keys of a `room_status` result, in delivery order.
fn letter_keys(room_status: &Value) -> Vec<String> {
    letters_of(room_status)
        .iter()
        .map(|l| {
            l["key"]
                .as_str()
                .unwrap_or_else(|| panic!("letter carries a key: {l}"))
                .to_string()
        })
        .collect()
}

/// The message of a JSON-RPC error response, asserting one is present.
fn error_message(resp: &Value) -> String {
    resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON-RPC error: {resp}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1 — stalled permission letters respect the 5-minute mark
// ---------------------------------------------------------------------------

#[test]
fn stalled_permission_letter_respects_the_five_minute_mark() {
    for (age_secs, expects_letter) in [(360, true), (240, false)] {
        let home = TempDir::new("home");
        let (repo, _head) = init_git_repo();
        let mut sup = supervisor(home.path(), repo.path());
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({
                "type": "permission_requested",
                "permission": "perm_1",
                "assignment": "asg_001",
                "detail": "Bash(git push --force) approval",
            }),
            NOW - age_secs,
        );

        let resp = sup.tools_call(json!(1), "room_status", json!({}));
        assert!(
            resp.get("error").is_none(),
            "room_status must answer: {resp}"
        );
        let letters = letters_of(&resp);
        if expects_letter {
            assert_eq!(
                letter_keys(&resp),
                vec!["att_stalled_permission_perm_1"],
                "one stalled-permission letter with a stable key (age {age_secs}s): {resp}"
            );
            let letter = &letters[0];
            assert_eq!(letter["kind"], "stalled-permission");
            assert_eq!(letter["ageSeconds"], json!(age_secs));
            assert!(
                letter["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("[hotpot attention att_stalled_permission_perm_1]")),
                "letter renders with the canonical attention prefix: {letter}"
            );
        } else {
            assert!(
                letters.is_empty(),
                "a permission only {age_secs}s old produces no letter: {resp}"
            );
            assert_eq!(
                resp["result"]["attention"]["allQuiet"],
                json!(true),
                "no letters means all quiet: {resp}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1 (handback half) — unread handbacks respect the
// 10-minute mark
// ---------------------------------------------------------------------------

#[test]
fn unread_handback_letter_respects_the_ten_minute_mark() {
    for (age_secs, expects_letter) in [(660, true), (540, false)] {
        let home = TempDir::new("home");
        let (repo, head) = init_git_repo();
        let id = dispatched_assignment(home.path(), repo.path(), &head);
        let mut sup = supervisor(home.path(), repo.path());
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({
                "type": "assignment_handed_back",
                "assignment": id,
                "correlation": "corr-fixture",
                "summary": "fixture handback awaiting Lead review",
            }),
            NOW - age_secs,
        );

        let resp = sup.tools_call(json!(1), "room_status", json!({}));
        assert!(
            resp.get("error").is_none(),
            "room_status must answer: {resp}"
        );
        if expects_letter {
            let expected_key = format!("att_unread_handback_{id}");
            assert_eq!(
                letter_keys(&resp),
                vec![expected_key.clone()],
                "one unread-handback letter for {id} (age {age_secs}s): {resp}"
            );
            let letter = &letters_of(&resp)[0];
            assert_eq!(letter["kind"], "unread-handback");
            assert_eq!(letter["ageSeconds"], json!(age_secs));
            assert!(
                letter["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with(&format!("[hotpot attention {expected_key}]"))),
                "letter renders with the canonical attention prefix: {letter}"
            );
        } else {
            assert!(
                letters_of(&resp).is_empty(),
                "a handback only {age_secs}s old produces no letter: {resp}"
            );
        }
    }
}

#[test]
fn a_handback_the_lead_already_accepted_is_not_unread() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let id = dispatched_assignment(home.path(), repo.path(), &head);
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({
            "type": "assignment_handed_back",
            "assignment": id,
            "correlation": "corr-fixture",
            "summary": "fixture handback",
        }),
        NOW - 3_600,
    );
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({"type": "assignment_accepted", "assignment": id}),
        NOW - 3_000,
    );
    let mut sup = supervisor(home.path(), repo.path());

    let resp = sup.tools_call(json!(1), "room_status", json!({}));
    assert!(
        letters_of(&resp).is_empty(),
        "an accepted handback is read, however old: {resp}"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 2 — NEEDS-HUMAN: / INCIDENT: lines from Lead
// ---------------------------------------------------------------------------

#[test]
fn needs_human_and_incident_lines_letter_but_ordinary_lead_messages_do_not() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    let mut sup = supervisor(home.path(), repo.path());
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({"type": "lead_message", "text": "wave 2 dispatched; gate green on asg_001"}),
        NOW - 600,
    );
    let ordinary = sup.tools_call(json!(1), "room_status", json!({}));
    assert!(
        letters_of(&ordinary).is_empty(),
        "an ordinary Lead status message produces no letter: {ordinary}"
    );

    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({"type": "lead_message", "text": "progress note\nNEEDS-HUMAN: release sign-off for the schema migration"}),
        NOW - 60,
    );
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({"type": "lead_message", "text": "INCIDENT: gate runner wedged after the power event"}),
        NOW - 30,
    );

    let resp = sup.tools_call(json!(2), "room_status", json!({}));
    let keys = letter_keys(&resp);
    assert_eq!(
        keys.len(),
        2,
        "one letter per marked line, still none for the ordinary message: {resp}"
    );
    assert!(
        keys.iter().all(|k| k.starts_with("att_needs_human_") || k.starts_with("att_incident_")),
        "letter keys carry their condition kind: {keys:?}"
    );
    for letter in letters_of(&resp) {
        let text = letter["text"].as_str().unwrap();
        assert!(
            text.starts_with("[hotpot attention att_"),
            "canonical prefix: {text}"
        );
        if letter["kind"] == "needs-human" {
            assert!(
                text.contains("release sign-off"),
                "the letter carries the NEEDS-HUMAN line: {text}"
            );
        } else {
            assert_eq!(letter["kind"], "incident");
            assert!(
                text.contains("gate runner wedged"),
                "the letter carries the INCIDENT line: {text}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 3 — Lead context rotation at >= 150k tokens
// ---------------------------------------------------------------------------

#[test]
fn rotation_letter_fires_at_one_fifty_one_k_but_not_one_forty_nine_k() {
    for (tokens, expects_letter) in [(151_000u64, true), (149_000u64, false)] {
        let home = TempDir::new("home");
        let (repo, _head) = init_git_repo();
        let mut sup = supervisor(home.path(), repo.path());
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({"type": "context_usage", "seat": "lead", "tokens": tokens}),
            NOW - 30,
        );

        let resp = sup.tools_call(json!(1), "room_status", json!({}));
        assert!(resp.get("error").is_none(), "room_status must answer: {resp}");
        if expects_letter {
            assert_eq!(
                letter_keys(&resp),
                vec!["att_lead_context_rotation"],
                "151k tokens crosses the rotation mark: {resp}"
            );
            let letter = &letters_of(&resp)[0];
            assert_eq!(letter["kind"], "lead-context-rotation");
            assert_eq!(letter["tokens"], json!(151_000u64));
            assert_eq!(letter["rotationMarkTokens"], json!(150_000u64));
        } else {
            assert!(
                letters_of(&resp).is_empty(),
                "149k tokens does not cross the rotation mark: {resp}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4 — repeated failures batch into one letter; the rate
// window suppresses immediate re-emission and admits it again after it passes
// ---------------------------------------------------------------------------

#[test]
fn three_consecutive_reds_letter_once_and_the_rate_window_gates_re_emission() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let id = dispatched_assignment(home.path(), repo.path(), &head);
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({
            "type": "assignment_handed_back",
            "assignment": id,
            "correlation": "corr-fixture",
            "summary": "fixture handback",
        }),
        NOW - 1_800,
    );
    for run in 1..=3u64 {
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({"type": "gate_started", "assignment": id, "gate": "./scripts/check.sh"}),
            NOW - 1_700 + run * 10,
        );
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({"type": "gate_completed", "assignment": id, "verdict": "red", "exitCode": 1}),
            NOW - 1_700 + run * 10 + 5,
        );
    }

    let expected_key = format!("att_repeated_failure_{id}");
    let mut sup = supervisor(home.path(), repo.path());

    // Three reds, one letter — not three.
    let first = sup.tools_call(json!(1), "room_status", json!({}));
    let failure_letters: Vec<Value> = letters_of(&first)
        .into_iter()
        .filter(|l| l["kind"] == "repeated-failure" || l["key"].as_str().is_some_and(|k| k.contains("repeated_failure")))
        .collect();
    assert_eq!(
        failure_letters.len(),
        1,
        "three consecutive reds produce exactly one repeated-failure letter: {first}"
    );
    assert_eq!(failure_letters[0]["key"], json!(expected_key));
    assert_eq!(failure_letters[0]["kind"], "repeated-failure");
    assert_eq!(failure_letters[0]["consecutiveReds"], json!(3));
    // The old handback is read (the Lead has acted: gates ran), so no unread letter mixes in.
    assert!(
        !letter_keys(&first).iter().any(|k| k.starts_with("att_unread_handback")),
        "gate activity means the handback is not unread: {first}"
    );

    // An immediate second room_status does not re-emit the same letter key.
    let second = sup.tools_call(json!(2), "room_status", json!({}));
    assert!(
        !letter_keys(&second).contains(&expected_key),
        "the rate window suppresses immediate re-emission: {second}"
    );

    // Past the rate window (default 600s) the key may be emitted again.
    let mut later = McpProcess::spawn_with_env(
        "supervisor",
        repo.path(),
        home.path(),
        None,
        &[("HOTPOT_NOW", &(NOW + 601).to_string())],
    );
    let third = later.tools_call(json!(1), "room_status", json!({}));
    assert!(
        letter_keys(&third).contains(&expected_key),
        "after the rate window passes the letter may re-emit: {third}"
    );
}

#[test]
fn two_consecutive_reds_do_not_raise_the_repeated_failure_letter() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let id = dispatched_assignment(home.path(), repo.path(), &head);
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({
            "type": "assignment_handed_back",
            "assignment": id,
            "correlation": "corr-fixture",
            "summary": "fixture handback",
        }),
        NOW - 1_800,
    );
    for run in 1..=2u64 {
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({"type": "gate_started", "assignment": id}),
            NOW - 1_700 + run * 10,
        );
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({"type": "gate_completed", "assignment": id, "verdict": "red"}),
            NOW - 1_700 + run * 10 + 5,
        );
    }
    let mut sup = supervisor(home.path(), repo.path());
    let resp = sup.tools_call(json!(1), "room_status", json!({}));
    assert!(
        letters_of(&resp).is_empty(),
        "the repeated-failure threshold is three consecutive reds, not two: {resp}"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5 — attention_feedback mute/snooze/downrank by letter key
// ---------------------------------------------------------------------------

/// Seeds two simultaneously-triggered conditions: a stalled permission and a
/// NEEDS-HUMAN line.
fn seed_two_noisy_conditions(home: &Path, repo: &Path) {
    seed_ledger_event_at(
        home,
        repo,
        json!({
            "type": "permission_requested",
            "permission": "perm_9",
            "detail": "WebFetch(exotic.host)",
        }),
        NOW - 600,
    );
    seed_ledger_event_at(
        home,
        repo,
        json!({"type": "lead_message", "text": "NEEDS-HUMAN: pick the migration window"}),
        NOW - 300,
    );
}

#[test]
fn muted_letter_keys_stay_silent_while_other_conditions_still_surface() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    seed_two_noisy_conditions(home.path(), repo.path());
    let mut sup = supervisor(home.path(), repo.path());

    let muted_key = "att_stalled_permission_perm_9".to_string();
    let feedback = sup.tools_call(
        json!(1),
        "attention_feedback",
        json!({"key": muted_key, "action": "mute"}),
    );
    assert!(
        feedback.get("error").is_none(),
        "attention_feedback mute must succeed: {feedback}"
    );

    let resp = sup.tools_call(json!(2), "room_status", json!({}));
    let keys = letter_keys(&resp);
    assert!(
        !keys.contains(&muted_key),
        "the muted key is omitted from the next batch: {resp}"
    );
    assert!(
        keys.iter().any(|k| k.starts_with("att_needs_human_")),
        "a different newly-triggered condition still appears: {resp}"
    );
}

#[test]
fn attention_feedback_rejects_unknown_actions_and_requires_a_key() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    let mut sup = supervisor(home.path(), repo.path());

    let bad_action = sup.tools_call(
        json!(1),
        "attention_feedback",
        json!({"key": "att_incident_seq1", "action": "shout"}),
    );
    let message = error_message(&bad_action);
    assert!(
        message.contains("shout") && message.contains("mute"),
        "the error names the invalid action and the permitted ones: {message}"
    );

    let no_key = sup.tools_call(json!(2), "attention_feedback", json!({"action": "mute"}));
    assert!(
        error_message(&no_key).contains("key"),
        "the error names the missing key: {no_key}"
    );
}

#[test]
fn snooze_suppresses_within_its_window_and_downrank_flags_the_letter() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    // The two seeded messages are ledger seq 1 and 2; letter keys are stable
    // functions of those seqs, so no discovery probe is needed (a probe would
    // consume the rate window for the very keys under test).
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({"type": "lead_message", "text": "NEEDS-HUMAN: snooze me"}),
        NOW - 120,
    );
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({"type": "lead_message", "text": "INCIDENT: rank me down"}),
        NOW - 60,
    );
    let needs_human = "att_needs_human_seq1".to_string();
    let incident_key = "att_incident_seq2".to_string();
    let mut sup = supervisor(home.path(), repo.path());

    let snoozed = sup.tools_call(
        json!(1),
        "attention_feedback",
        json!({"key": needs_human, "action": "snooze", "forSeconds": 300}),
    );
    assert!(
        snoozed.get("error").is_none(),
        "snooze must succeed: {snoozed}"
    );
    let downranked = sup.tools_call(
        json!(2),
        "attention_feedback",
        json!({"key": incident_key, "action": "downrank"}),
    );
    assert!(
        downranked.get("error").is_none(),
        "downrank must succeed: {downranked}"
    );

    // Inside the snooze window: the snoozed key is gone, the downranked one
    // still surfaces, flagged, and last.
    let during = sup.tools_call(json!(3), "room_status", json!({}));
    let letters = letters_of(&during);
    let keys = letters
        .iter()
        .map(|l| l["key"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        !keys.contains(&needs_human),
        "the snoozed key is suppressed within its window: {during}"
    );
    assert_eq!(
        keys.last(),
        Some(&incident_key),
        "the downranked key still surfaces, sorted last: {during}"
    );
    assert_eq!(
        letters.last().unwrap()["downranked"],
        json!(true),
        "the downranked letter is flagged: {during}"
    );

    // Past the snooze window the key may surface again (it was never delivered
    // while snoozed, so the rate window does not hold it back either).
    let mut after = McpProcess::spawn_with_env(
        "supervisor",
        repo.path(),
        home.path(),
        None,
        &[("HOTPOT_NOW", &(NOW + 301).to_string())],
    );
    let resp = after.tools_call(json!(1), "room_status", json!({}));
    assert!(
        letter_keys(&resp).contains(&needs_human),
        "past the snooze window the key surfaces again: {resp}"
    );
}

// ---------------------------------------------------------------------------
// Letters also surface in runtime_findings (read-only, batch not consumed)
// ---------------------------------------------------------------------------

#[test]
fn runtime_findings_carries_attention_letters_without_consuming_the_batch() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    let mut sup = supervisor(home.path(), repo.path());
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({
            "type": "permission_requested",
            "permission": "perm_f",
            "detail": "Bash(cargo publish)",
        }),
        NOW - 400,
    );

    let findings = sup.tools_call(json!(1), "runtime_findings", json!({}));
    assert!(findings.get("error").is_none(), "runtime_findings must answer: {findings}");
    let attention: Vec<Value> = findings["result"]["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("findings array: {findings}"))
        .iter()
        .filter(|f| f["kind"] == "attention" || f["key"].as_str().is_some_and(|k| k.starts_with("att_")))
        .cloned()
        .collect();
    assert_eq!(
        attention.len(),
        1,
        "the letter surfaces as a finding: {findings}"
    );
    assert_eq!(attention[0]["key"], "att_stalled_permission_perm_f");
    assert!(
        attention[0]["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("[hotpot attention att_stalled_permission_perm_f]")),
        "the finding renders the canonical letter text: {attention:?}"
    );

    // runtime_findings is read-only: the letter is still due for room_status
    // (no attention_delivered event was recorded on its behalf).
    let status = sup.tools_call(json!(2), "room_status", json!({}));
    assert!(
        letter_keys(&status).contains(&"att_stalled_permission_perm_f".to_string()),
        "read-only findings did not consume the delivery batch: {status}"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 7 — message_lead reaches the Lead
// ---------------------------------------------------------------------------

/// ASCII unit separator the fake `hc` uses between recorded argv elements.
const ARG_SEP: char = '\u{1f}';

/// Writes the fake `hc` runner (ticket 06 pattern): `hc ls --json` prints the
/// scripted `<state>/list.json`; `hc run ...` and `hc send <target> <message>`
/// record their argv (the leading subcommand word replaced by a marker word) as
/// unit-separator-separated lines appended to `<state>/create.log` and
/// `<state>/send.log`.
fn write_fake_hc(bin_dir: &Path) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let script = "\
#!/bin/sh\n\
state=\"$HOTPOT_FAKE_HC_STATE\"\n\
case \"$1\" in\n\
  ls) exec cat \"$state/list.json\" ;;\n\
  run) shift; { printf 'run'; for a in \"$@\"; do printf '\\037%s' \"$a\"; done; printf '\\n'; } >> \"$state/create.log\" ;;\n\
  send) shift; { printf 'send'; for a in \"$@\"; do printf '\\037%s' \"$a\"; done; printf '\\n'; } >> \"$state/send.log\" ;;\n\
esac\n\
exit 0\n";
    let shim = bin_dir.join("hc");
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Parses the fake `hc` send log into argv vectors (leading `send` word kept).
fn recorded_sends(log: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            line.split(ARG_SEP)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .collect()
}

#[test]
fn message_lead_records_a_mailbox_event_and_sends_via_hc() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    let bin = TempDir::new("hcbin");
    let hc_state = TempDir::new("hcstate");
    write_fake_hc(bin.path());
    // A live lead terminal for the Project: message_lead addresses it by label.
    std::fs::write(
        hc_state.path().join("list.json"),
        json!([{
            "address": "term-lead-9",
            "status": "live",
            "labels": {
                "hotpot.v1": "true",
                "hotpot.project": common::expected_slug(repo.path()),
                "hotpot.role": "lead",
                "hotpot.correlation": "codex-lead",
            },
        }])
        .to_string()
        + "\n",
    )
    .unwrap();
    let mut sup = McpProcess::spawn_with_env(
        "supervisor",
        repo.path(),
        home.path(),
        None,
        &[
            ("HOTPOT_NOW", &NOW.to_string()),
            ("HOTPOT_HC", bin.path().join("hc").to_str().unwrap()),
            ("HOTPOT_FAKE_HC_STATE", hc_state.path().to_str().unwrap()),
        ],
    );

    let resp = sup.tools_call(
        json!(1),
        "message_lead",
        json!({"message": "Human directive: ship the attention slice before Friday"}),
    );
    assert!(
        resp.get("error").is_none(),
        "message_lead must succeed: {resp}"
    );
    assert_eq!(
        resp["result"]["hcSend"]["delivered"],
        json!(true),
        "delivery reports the addressed terminal: {resp}"
    );
    assert_eq!(resp["result"]["hcSend"]["target"], "term-lead-9");

    // The durable mailbox event is observable in the ledger.
    let mailbox: Vec<Value> = common::ledger_lines(home.path())
        .into_iter()
        .filter(|e| e["type"] == json!("message_to_lead"))
        .collect();
    assert_eq!(mailbox.len(), 1, "exactly one mailbox event: {mailbox:?}");
    assert_eq!(mailbox[0]["from"], "supervisor");
    assert_eq!(
        mailbox[0]["message"],
        "Human directive: ship the attention slice before Friday"
    );

    // ... and the fake hc send log carries the same message to the terminal.
    let sends = recorded_sends(&hc_state.path().join("send.log"));
    assert_eq!(sends.len(), 1, "one hc send: {sends:?}");
    assert_eq!(sends[0][1], "term-lead-9", "sent to the lead terminal: {sends:?}");
    assert!(
        sends[0][2].contains("ship the attention slice"),
        "the message text rides the send: {sends:?}"
    );
}

#[test]
fn message_lead_without_hc_still_records_the_durable_mailbox_event() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    // No hc anywhere (a sandboxed PATH without one, no HOTPOT_HC).
    let mut sup = McpProcess::spawn_with_env(
        "supervisor",
        repo.path(),
        home.path(),
        None,
        &[("HOTPOT_NOW", &NOW.to_string()), ("PATH", "/usr/bin:/bin")],
    );

    let resp = sup.tools_call(json!(1), "message_lead", json!({"message": "offline note"}));
    assert!(
        resp.get("error").is_none(),
        "the durable event must still be recorded: {resp}"
    );
    assert_eq!(
        resp["result"]["hcSend"]["delivered"],
        json!(false),
        "delivery honestly reports hc being unavailable: {resp}"
    );
    assert_eq!(
        common::ledger_lines(home.path())
            .iter()
            .filter(|e| e["type"] == json!("message_to_lead"))
            .count(),
        1,
        "the mailbox event is recorded without hc"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 6 — quiet-point Lead replacement
// ---------------------------------------------------------------------------

/// Spawns a supervisor session over the fake `hc` runner and a fake vendor
/// binary on PATH (the ticket-06 launcher seam), with the codex Lead Role Home
/// a prior `hotpot setup --apply` would have written.
struct ReplacementFixture {
    home: TempDir,
    _repo_guard: TempDir,
    repo: std::path::PathBuf,
    hc_state: TempDir,
    bin: TempDir,
}

impl ReplacementFixture {
    fn new(tag: &str) -> Self {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        let home = TempDir::new(&format!("rep-home-{tag}"));
        let bin = TempDir::new(&format!("rep-bin-{tag}"));
        let hc_state = TempDir::new(&format!("rep-hc-{tag}"));
        let (repo_guard, _head) = init_git_repo();
        // No live terminals: every launch creates a fresh successor terminal.
        fs::write(hc_state.path().join("list.json"), "[]\n").unwrap();
        write_fake_hc(bin.path());
        // The fake vendor CLI the 06 launcher resolves on PATH.
        let shim = bin.path().join("codex");
        fs::write(&shim, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        // The Lead Role Home a prior `hotpot setup --apply` wrote.
        fs::create_dir_all(home.path().join("roles").join("codex").join("lead")).unwrap();
        let repo = repo_guard.path().to_path_buf();
        Self {
            home,
            _repo_guard: repo_guard,
            repo,
            hc_state,
            bin,
        }
    }

    fn supervisor(&self) -> McpProcess {
        let path = format!("{}:{}", self.bin.path().display(), std::env::var("PATH").unwrap());
        McpProcess::spawn_with_env(
            "supervisor",
            &self.repo,
            self.home.path(),
            None,
            &[
                ("HOTPOT_NOW", &NOW.to_string()),
                ("HOTPOT_HC", self.bin.path().join("hc").to_str().unwrap()),
                ("HOTPOT_FAKE_HC_STATE", self.hc_state.path().to_str().unwrap()),
                ("PATH", &path),
            ],
        )
    }

    /// The recorded `hc run` argv of every launch (leading `run` word stripped).
    fn create_invocations(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.hc_state.path().join("create.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                line.trim_start_matches(ARG_SEP)
                    .split(ARG_SEP)
                    .filter(|s| !s.is_empty())
                    .skip(1)
                    .map(str::to_string)
                    .collect()
            })
            .collect()
    }
}

#[test]
fn lead_replace_prepare_refuses_while_writable_work_is_live() {
    let fixture = ReplacementFixture::new("busy");
    let id = dispatched_assignment(fixture.home.path(), &fixture.repo, &head_of(&fixture.repo));
    let mut sup = fixture.supervisor();

    let resp = sup.tools_call(json!(1), "lead_replace_prepare", json!({}));
    let message = error_message(&resp);
    assert!(
        message.to_lowercase().contains("quiet"),
        "the refusal names the quiet-point requirement: {message}"
    );
    assert!(
        message.contains(&id),
        "the refusal names the blocking assignment: {message}"
    );
    // Nothing was prepared: a confirm right after has nothing to act on.
    let confirm = sup.tools_call(
        json!(2),
        "lead_replace_confirm",
        json!({"handoffText": "edited"}),
    );
    assert!(
        error_message(&confirm).contains("prepare"),
        "confirm without a prepared replacement is refused: {confirm}"
    );
}

/// The repo HEAD as a helper (fixture repos have exactly one commit).
fn head_of(repo: &Path) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn lead_replace_prepare_at_a_quiet_point_returns_a_structured_draft() {
    let fixture = ReplacementFixture::new("quiet");
    let head = head_of(&fixture.repo);
    let id = dispatched_assignment(fixture.home.path(), &fixture.repo, &head);
    // The assignment is cleanly handed back: the room is at a quiet point even
    // though the handback still awaits the successor Lead's integration.
    seed_ledger_event_at(
        fixture.home.path(),
        &fixture.repo,
        json!({
            "type": "assignment_handed_back",
            "assignment": id,
            "correlation": "corr-fixture",
            "summary": "slice handed back, gate green",
            "candidate": "abc123",
        }),
        NOW - 60,
    );
    let mut sup = fixture.supervisor();

    let resp = sup.tools_call(json!(1), "lead_replace_prepare", json!({}));
    assert!(
        resp.get("error").is_none(),
        "a cleanly handed-back room is quiet: {resp}"
    );
    let result = &resp["result"];
    assert_eq!(result["replacement"], "lrep_001");
    assert_eq!(result["quietPoint"]["verified"], json!(true));
    let text = result["handoffText"].as_str().unwrap();
    assert!(
        text.contains(&id) && text.contains("HandedBack"),
        "the draft summarizes the assignment board: {text}"
    );
    assert!(
        result["assignments"][0]["awaitingIntegration"] == json!(true),
        "the draft flags work awaiting the successor's integration: {result}"
    );

    // The draft is durable: a fresh session confirms against the same replacement.
    let mut fresh = fixture.supervisor();
    let again = fresh.tools_call(json!(2), "lead_replace_prepare", json!({}));
    assert!(
        error_message(&again).contains("lrep_001"),
        "a second prepare while one is in flight names it: {again}"
    );
}

#[test]
fn lead_replace_confirm_archives_launches_the_successor_and_seeds_it() {
    let fixture = ReplacementFixture::new("confirm");
    let head = head_of(&fixture.repo);
    let id = dispatched_assignment(fixture.home.path(), &fixture.repo, &head);
    seed_ledger_event_at(
        fixture.home.path(),
        &fixture.repo,
        json!({
            "type": "assignment_handed_back",
            "assignment": id,
            "correlation": "corr-fixture",
            "summary": "slice handed back",
            "candidate": "abc123",
        }),
        NOW - 60,
    );
    let mut sup = fixture.supervisor();
    let prepared = sup.tools_call(json!(1), "lead_replace_prepare", json!({}));
    assert!(prepared.get("error").is_none(), "prepare: {prepared}");

    let edited = "Edited handoff: integrate asg_001 first, then read WORKSPACE_PROTOCOL.md";
    let confirmed = sup.tools_call(
        json!(2),
        "lead_replace_confirm",
        json!({"handoffText": edited, "agent": "codex"}),
    );
    assert!(
        confirmed.get("error").is_none(),
        "confirm must succeed: {confirmed}"
    );
    let result = &confirmed["result"];
    assert_eq!(result["state"], "confirmed");
    assert_eq!(result["successor"]["agent"], "codex");
    assert_eq!(result["successor"]["correlation"], "codex-lead");

    // The successor Lead was launched via the fake hc with the project labels.
    let invocations = fixture.create_invocations();
    assert_eq!(
        invocations.len(),
        1,
        "exactly one successor terminal launched: {invocations:?}"
    );
    let argv = &invocations[0];
    let slug = common::expected_slug(&fixture.repo);
    for label in [
        "hotpot.v1=true".to_string(),
        format!("hotpot.project={slug}"),
        "hotpot.role=lead".to_string(),
        "hotpot.agent=codex".to_string(),
        "hotpot.correlation=codex-lead".to_string(),
    ] {
        assert!(
            argv.windows(2).any(|w| w[0] == "--label" && w[1] == label),
            "the successor launch carries label `{label}`: {argv:?}"
        );
    }

    // The edited handoff text is observable in the successor's seed record.
    let seed_path = std::path::PathBuf::from(result["seedRecord"].as_str().unwrap());
    assert!(
        seed_path
            .to_string_lossy()
            .contains("lead-replacements/lrep_001-seed.md"),
        "the seed record lives under the project runtime dir: {}",
        seed_path.display()
    );
    let seeded = std::fs::read_to_string(&seed_path).unwrap();
    assert_eq!(seeded, edited, "the seed record carries the edited text");

    // The ledger records the archive and the confirmed replacement.
    let events = common::ledger_lines(fixture.home.path());
    assert!(
        events.iter().any(|e| e["type"] == json!("lead_archived")
            && e["replacement"] == json!("lrep_001")),
        "the old lead is archived: {events:?}"
    );
    let confirmed_events: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == json!("lead_replace_confirmed"))
        .collect();
    assert_eq!(confirmed_events.len(), 1);
    assert_eq!(confirmed_events[0]["handoffText"], json!(edited));
    assert_eq!(confirmed_events[0]["successorAgent"], "codex");

    // Confirm again has nothing in flight to act on.
    let again = sup.tools_call(
        json!(3),
        "lead_replace_confirm",
        json!({"handoffText": edited}),
    );
    assert!(
        error_message(&again).contains("prepare"),
        "a second confirm is refused: {again}"
    );
}

#[test]
fn lead_replace_cancel_aborts_and_leaves_the_old_lead_active() {
    let fixture = ReplacementFixture::new("cancel");
    let mut sup = fixture.supervisor();
    let prepared = sup.tools_call(json!(1), "lead_replace_prepare", json!({}));
    assert!(prepared.get("error").is_none(), "prepare: {prepared}");

    let cancelled = sup.tools_call(json!(2), "lead_replace_cancel", json!({}));
    assert!(
        cancelled.get("error").is_none(),
        "cancel must succeed: {cancelled}"
    );
    assert_eq!(cancelled["result"]["state"], "cancelled");
    assert_eq!(cancelled["result"]["lead"], "active");

    // Nothing was launched and nothing was archived.
    assert!(
        fixture.create_invocations().is_empty(),
        "cancel launches no successor"
    );
    let events = common::ledger_lines(fixture.home.path());
    assert!(
        !events.iter().any(|e| e["type"] == json!("lead_archived")),
        "the old lead stays active: {events:?}"
    );
    assert!(
        events.iter().any(|e| e["type"] == json!("lead_replace_cancelled")),
        "the cancellation is recorded: {events:?}"
    );

    // A confirm after a cancel has nothing in flight; a fresh prepare starts over.
    let confirm = sup.tools_call(
        json!(3),
        "lead_replace_confirm",
        json!({"handoffText": "too late"}),
    );
    assert!(
        error_message(&confirm).contains("prepare"),
        "confirm after cancel is refused: {confirm}"
    );
    let fresh = sup.tools_call(json!(4), "lead_replace_prepare", json!({}));
    assert!(
        fresh.get("error").is_none(),
        "a new prepare may start after a cancel: {fresh}"
    );
    assert_eq!(fresh["result"]["replacement"], "lrep_002");
}

#[test]
fn the_failure_threshold_is_configurable_through_the_environment() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let id = dispatched_assignment(home.path(), repo.path(), &head);
    seed_ledger_event_at(
        home.path(),
        repo.path(),
        json!({"type": "assignment_handed_back", "assignment": id, "summary": "fixture"}),
        NOW - 1_800,
    );
    for run in 1..=2u64 {
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({"type": "gate_started", "assignment": id}),
            NOW - 100 + run,
        );
        seed_ledger_event_at(
            home.path(),
            repo.path(),
            json!({"type": "gate_completed", "assignment": id, "verdict": "red"}),
            NOW - 100 + run + 1,
        );
    }
    // The default threshold (3) would stay quiet at two reds; an operator (or
    // test) lowering it to 2 raises the letter.
    let mut strict = McpProcess::spawn_with_env(
        "supervisor",
        repo.path(),
        home.path(),
        None,
        &[
            ("HOTPOT_NOW", &NOW.to_string()),
            ("HOTPOT_ATTENTION_FAILURE_THRESHOLD", "2"),
        ],
    );
    let resp = strict.tools_call(json!(1), "room_status", json!({}));
    let expected = format!("att_repeated_failure_{id}");
    assert_eq!(
        letter_keys(&resp),
        vec![expected],
        "two reds letter under a lowered threshold: {resp}"
    );
    assert_eq!(
        letters_of(&resp)[0]["threshold"],
        json!(2),
        "the letter reports the configured threshold: {resp}"
    );
}

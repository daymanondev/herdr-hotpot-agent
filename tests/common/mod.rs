//! Shared seam harness for the Runtime Coordinator Layer tests (`hotpot mcp`).
//!
//! Each test spawns the cargo-built `hotpot mcp` binary as a child process against a
//! temporary `HOTPOT_HOME` and a temporary real git repository, drives JSON-RPC 2.0
//! request lines over stdio, asserts each response line, then asserts the on-disk
//! ledger under `<HOTPOT_HOME>/runtime/v1/<project-slug>/ledger.jsonl`.
//! Zero network; no private-internal assertions.

#![allow(dead_code)]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Temporary directory guard: removed recursively on drop. Names combine the
/// process id, a monotonic counter (the OS clock alone can tie across threads),
/// and a timestamp, so parallel tests never share scratch state.
static DIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let n = DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "hotpot-rt-{tag}-{}-{n}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `git -C <repo> <args..>` and returns trimmed stdout, asserting success.
pub fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git binary spawns");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Runs `git -C <repo> <args..>` without asserting success; returns (ok, stdout, stderr).
pub fn git_soft(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git binary spawns");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

/// Creates a temporary real git repository with one commit; returns (dir, HEAD sha).
pub fn init_git_repo() -> (TempDir, String) {
    let repo = TempDir::new("repo");
    let path = repo.path();
    git(path, &["init", "-q"]);
    git(path, &["config", "user.email", "hotpot-test@example.com"]);
    git(path, &["config", "user.name", "Hotpot Test"]);
    std::fs::write(path.join("README.md"), "# fixture project\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "fixture initial commit"]);
    let head = git(path, &["rev-parse", "HEAD"]);
    (repo, head)
}

/// A spawned `hotpot mcp` child process speaking JSON-RPC 2.0 over stdio.
pub struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<String>,
}

impl McpProcess {
    pub fn spawn(role: &str, project: &Path, home: &Path) -> Self {
        Self::spawn_correlated(role, project, home, None)
    }

    /// Spawns `hotpot mcp` with the session correlation pin (`--correlation`),
    /// the server-side identity a peer seat derives its assignment binding from.
    pub fn spawn_correlated(
        role: &str,
        project: &Path,
        home: &Path,
        correlation: Option<&str>,
    ) -> Self {
        Self::spawn_with_env(role, project, home, correlation, &[])
    }

    /// Spawns `hotpot mcp` with extra environment variables — the injectable
    /// seams the attention and replacement slices ride on (`HOTPOT_NOW` clock,
    /// `HOTPOT_HC` fake runner, a sandboxed `PATH` with fake vendor binaries).
    pub fn spawn_with_env(
        role: &str,
        project: &Path,
        home: &Path,
        correlation: Option<&str>,
        envs: &[(&str, &str)],
    ) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hotpot"));
        cmd.args(["mcp", "--role", role])
            .arg("--project")
            .arg(project)
            .env("HOTPOT_HOME", home);
        for (key, value) in envs {
            cmd.env(key, value);
        }
        if let Some(correlation) = correlation {
            cmd.arg("--correlation").arg(correlation);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn hotpot mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            rx,
        }
    }

    /// Writes one raw request line and returns the next response line parsed as JSON.
    pub fn raw_roundtrip(&mut self, raw: &str) -> Value {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{raw}").unwrap();
        stdin.flush().unwrap();
        let line = self
            .rx
            .recv_timeout(Duration::from_secs(30))
            .expect("hotpot mcp answered within 30s");
        serde_json::from_str(&line).unwrap_or(Value::Null)
    }

    pub fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        let mut line = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            line["params"] = params;
        }
        self.raw_roundtrip(&line.to_string())
    }

    pub fn tools_call(&mut self, id: Value, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.flush();
        }
        // Closing stdin ends the server's read loop; reap the child.
        let _ = self.child.wait();
    }
}

/// Returns the single project ledger path under `<home>/runtime/v1/<project-slug>/`.
pub fn ledger_path(home: &Path) -> PathBuf {
    let v1 = home.join("runtime").join("v1");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&v1).expect("runtime/v1 exists after hotpot mcp starts") {
        let entry = entry.unwrap();
        let candidate = entry.path().join("ledger.jsonl");
        if candidate.exists() {
            found.push(candidate);
        }
    }
    assert_eq!(
        found.len(),
        1,
        "expected exactly one project ledger, got {found:?}"
    );
    found[0].clone()
}

/// Parses the ledger file into JSON values, asserting every line is valid JSON.
pub fn ledger_lines(home: &Path) -> Vec<Value> {
    let path = ledger_path(home);
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line parses as JSON"))
        .collect()
}

/// Number of ledger lines, or 0 when the ledger file has not been created yet.
pub fn ledger_line_count(home: &Path) -> usize {
    let v1 = home.join("runtime").join("v1");
    let Ok(entries) = std::fs::read_dir(&v1) else {
        return 0;
    };
    let mut count = 0usize;
    for entry in entries.flatten() {
        let ledger = entry.path().join("ledger.jsonl");
        if ledger.exists() {
            count += std::fs::read_to_string(&ledger)
                .unwrap()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();
        }
    }
    count
}

/// Appends an event line to the ledger the way a later runtime slice (peer `ask` /
/// `handoff`, ticket 09) will write it: `seq` continues the strictly increasing count.
/// Like the engine's `Ledger::append`, the line and its newline go out in one
/// `write` — the engine or the detached gate runner may be appending to the
/// same file while the test reads it, and `writeln!`'s separate newline write
/// is readable as a torn line in between.
pub fn seed_ledger_event(home: &Path, mut event: Value) {
    let path = ledger_path(home);
    let next = ledger_lines(home)
        .iter()
        .filter_map(|e| e.get("seq").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        + 1;
    event["seq"] = json!(next);
    event["ts"] = json!(0);
    append_line(&path, &event.to_string());
}

/// The canonical `<basename>-<sha1(gitRoot)[0..6]>` slug of `repo`, computed through
/// `shasum` (an SHA-1 implementation independent of the Rust engine's own) so tests
/// can disagree with the product's `runtime::project_slug`.
pub fn expected_slug(repo: &Path) -> String {
    let canonical = repo.canonicalize().unwrap();
    let base = canonical.file_name().unwrap().to_str().unwrap();
    let out = Command::new("shasum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
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

/// Appends a hand-written fixture event to the project ledger with an explicit
/// unix-seconds `ts` (the injectable-clock domain) — the seam the attention
/// detection consumes (ticket 11). Creates the ledger file if it does not exist
/// yet, so pure-fixture ledgers need no earlier MCP append.
pub fn seed_ledger_event_at(home: &Path, repo: &Path, mut event: Value, ts: u64) {
    let dir = home
        .join("runtime")
        .join("v1")
        .join(expected_slug(repo));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ledger.jsonl");
    let next = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|e| e.get("seq").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        + 1;
    event["seq"] = json!(next);
    event["ts"] = json!(ts);
    append_line(&path, &event.to_string());
}

/// Appends `line` plus its newline in one write on an append-mode file — the
/// same single-write discipline as the engine's `Ledger::append`.
fn append_line(path: &Path, line: &str) {
    use std::io::Write;
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(buf.as_bytes()).unwrap();
}

/// Valid story-19 `assignment_create` arguments against the given verified commit.
pub fn valid_create(head: &str) -> Value {
    json!({
        "outcome": "Ship the append-only runtime ledger",
        "disposition": "Engineer",
        "baseCommit": head,
        "writeScope": ["src/**"],
        "serialOnly": ["Cargo.lock"],
        "gate": "./scripts/check.sh",
        "thinking": "medium",
        "thinkingReason": "bounded mechanical change to the dispatcher"
    })
}

/// Creates a Draft assignment and returns its `asg_*` id.
pub fn create_assignment(mcp: &mut McpProcess, args: Value) -> String {
    let resp = mcp.tools_call(json!("create"), "assignment_create", args);
    assert!(
        resp.get("error").is_none(),
        "assignment_create must succeed: {resp}"
    );
    resp["result"]["id"].as_str().unwrap().to_string()
}

/// Invokes `hotpot hook` for a correlated peer seat with a PreToolUse payload and
/// returns the parsed verdict, asserting the process exited 0.
pub fn peer_hook(home: &Path, correlation: &str, payload: &Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args(["hook", "--role", "peer", "--correlation", correlation])
        .arg("--state-dir")
        .arg(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hotpot hook");
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{payload}").unwrap();
        stdin.flush().unwrap();
    }
    // Closing stdin lets the hook's read_to_string finish.
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("hotpot hook exits");
    assert!(
        out.status.success(),
        "hotpot hook exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

/// A PreToolUse payload for a file-write tool touching `path` inside a workspace.
pub fn write_payload(path: &Path) -> Value {
    json!({
        "tool_name": "Write",
        "tool_input": {"file_path": path, "content": "fixture\n"}
    })
}

/// A PreToolUse payload for a Bash tool running `command`.
pub fn bash_payload(command: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": command}})
}

/// Commits `rel` with `content` inside `repo` and returns the new HEAD sha.
pub fn commit_file(repo: &Path, rel: &str, content: &str) -> String {
    let file = repo.join(rel);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&file, content).unwrap();
    git(repo, &["add", "--", rel]);
    git(repo, &["commit", "-qm", &format!("fixture: touch {rel}")]);
    git(repo, &["rev-parse", "HEAD"])
}

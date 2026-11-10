//! Append-only runtime event ledger under `<HOTPOT_HOME>/runtime/v1/<project-slug>/ledger.jsonl`.
//!
//! The ledger is the single source of truth for runtime state: one JSON object per
//! line, each carrying a strictly increasing `seq` (starting at 1) and a unix-seconds
//! `ts`. The file only ever grows — projections (e.g. `AssignmentState`) are rebuilt
//! from the log on every read, never stored in a second authoritative database.
//!
//! Event types written by this slice (`assignment` = assignment id):
//! - `assignment_created`    { assignment, outcome, disposition, baseCommit, writeScope, serialOnly, gate, thinking?, thinkingReason? }
//! - `assignment_dispatched` { assignment, correlation, workspace, agent?, model? }
//! - `writer_lease_acquired` { assignment, mode, worktree?, cargoTargetDir?, worktreeInclude? }
//! - `writer_lease_released` { assignment, worktreeAction }
//! - `writer_lease_reclaimed` { assignment, mode, worktreeAction }
//! - `assignment_asked`      { assignment, correlation, question, signal? } (written by the peer `ask` path)
//! - `assignment_answered`   { assignment, answer }
//! - `workspace_write`       { assignment, correlation, workspace, tool }   (runtime evidence, `hotpot hook`)
//! - `workspace_gate`        { assignment, correlation, workspace, gate }   (runtime evidence, `hotpot hook`)
//! - `assignment_handed_back`{ assignment, correlation, summary, candidate?, baseCommit?, scopeViolations? } (written by the peer `handoff` path)
//! - `gate_started`          { assignment, gate }
//! - `gate_completed`        { assignment, verdict, durationMs, exitCode?, reason?, tail? } (written by the detached gate runner; `tail` is the masked red tail path under Hotpot Home)
//! - `assignment_accepted`   { assignment, note? }
//! - `assignment_rejected`   { assignment, note? }
//! - `assignment_stopped`    { assignment }
//! - `assignment_abandoned`  { assignment }
//! - `assignment_closed`     { assignment }
//!
//! Unknown event types and unparsable lines are skipped on replay so newer runtime
//! versions never corrupt older projections (preserve-only across setup runs).

use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Ledger I/O failure surfaced as a JSON-RPC internal error by the dispatcher.
#[derive(Debug)]
pub struct LedgerError(pub String);

impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "runtime ledger error: {}", self.0)
    }
}

impl From<std::io::Error> for LedgerError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

/// Canonical `Project` slug: `<basename>-<sha1(gitRoot)[0..6]>` (spec: 1:1 Project
/// Topology on Herdr). The digest keeps two same-named checkouts apart.
pub fn project_slug(project_root: &Path) -> String {
    let canonical = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let base = canonical
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    format!(
        "{base}-{}",
        &sha1_hex(canonical.to_string_lossy().as_bytes())[..6]
    )
}

/// Pure SHA-1 over `data`, hex-encoded. Used only as a deterministic slug digest
/// (not for security); implemented locally so the runtime needs no extra crates.
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999_u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        for (hi, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *hi = hi.wrapping_add(v);
        }
    }

    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// The append-only JSONL event log for one `Project`.
pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    /// Opens (creating directories as needed) the ledger for `project_root`.
    pub fn open(state_dir: &Path, project_root: &Path) -> Result<Self, LedgerError> {
        let dir = state_dir
            .join("runtime")
            .join("v1")
            .join(project_slug(project_root));
        fs::create_dir_all(&dir)?;
        Ok(Self {
            path: dir.join("ledger.jsonl"),
        })
    }

    /// Wraps an existing ledger file path (used by the evidence recorder, which
    /// discovers ledgers under `runtime/v1/` without a project root). Appending
    /// creates the file only if it does not exist yet.
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Path of the ledger file (used by tests and diagnostics).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The per-project runtime dir (`<state>/runtime/v1/<slug>`) hosting the
    /// ledger, isolated worktrees, and the shared warm-build cache.
    pub fn project_dir(&self) -> &Path {
        self.path
            .parent()
            .expect("ledger path always sits inside its project dir")
    }

    /// Reads every parsable event line, in file order. Unparsable lines (e.g. a
    /// torn final write after a crash) are skipped rather than fatal.
    pub fn read_events(&self) -> Vec<Value> {
        let Ok(file) = fs::File::open(&self.path) else {
            return Vec::new();
        };
        BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect()
    }

    /// Appends one event with the next strictly increasing `seq` and a unix `ts`.
    /// The file is opened in append mode only — the ledger never shrinks.
    pub fn append(&self, mut event: Value) -> Result<u64, LedgerError> {
        let next = self.next_seq();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        event["seq"] = json!(next);
        event["ts"] = json!(ts);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{event}")?;
        file.flush()?;
        Ok(next)
    }

    fn next_seq(&self) -> u64 {
        self.read_events()
            .iter()
            .filter_map(|e| e.get("seq").and_then(Value::as_u64))
            .max()
            .unwrap_or(0)
            + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_known_vectors() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        // > 64-byte input crossing the padding boundary.
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }
}

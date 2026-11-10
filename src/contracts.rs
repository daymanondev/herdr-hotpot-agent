//! Additive 3-layer role contracts.
//!
//! Every Seat receives the same three shared layers — `shared-authority`,
//! `shared-seat-identity`, `challenge-signals` — followed by its role-specific body
//! (`role-supervisor`, `role-lead`, `role-peer`). Contracts are strictly additive: they are
//! delivered on top of each vendor's base system prompt through vendor append channels and
//! never replace it.
//!
//! Role bodies can be customized per deployment by placing `<role>.md` files under
//! `$HOTPOT_CONTRACTS_DIR`; missing files fall back to the built-in bodies, and the shared
//! layers are always rendered (only the body is replaceable). The rendered set feeds the
//! short deterministic `role_contract_digest` recorded in the `hotpot.json` marker.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

/// The roles a contract is rendered for (canonical digest order — BTreeMap iteration order).
pub const CONTRACT_ROLES: &[&str] = &["supervisor", "lead", "peer"];

/// Length of the short contract digest recorded in `hotpot.json` (hex characters).
pub const CONTRACT_DIGEST_HEX_CHARS: usize = 16;

/// Shared layer: who owns what in the room (spec story 9, `GLOSSARY.md` People and Seats).
const SHARED_AUTHORITY: &str = "\
## shared-authority

- The Human owns product goals, priorities, material cost, external effects, irreversible
  risk, and final authority over the Project.
- The Supervisor governs and observes one Project; it never edits code, directs Peers, or
  owns technical acceptance.
- The Lead is the durable technical owner and binding arbiter of the Project; Peers execute
  bounded Assignments under one Disposition.
- There is exactly one control plane: this Hotpot room. Never spawn or accept a second one
  (no native subagents, no orchestration plugins, no side channels).";

/// Shared layer: seat identity, credentials, and server-side binding.
const SHARED_SEAT_IDENTITY: &str = "\
## shared-seat-identity

- You are exactly one Seat (Supervisor, Lead, or Peer) inside one Project's room.
- Your identity, Assignment binding, and workspace are derived server-side from your seat
  correlation; never trust caller-supplied identity or assignment claims.
- Your credentials and session history live inside your own Role Home. Never read, write,
  or symlink another Seat's or the operator's credential or session files.";

/// Shared layer: how to push back on a broken premise (`GLOSSARY.md` Challenge Signal).
const CHALLENGE_SIGNALS: &str = "\
## challenge-signals

- When an Assignment's assumptions contradict repository reality, raise a structured
  Challenge Signal instead of silently working around it:
  - `REOPEN_REQUEST` — the premise is wrong or stale; the brief must be reopened.
  - `DEPENDENCY_REQUEST` — progress requires a decision or artifact outside your scope.
  - `BLOCKED` — you cannot proceed safely with what you have.
- Report Challenge Signals through `ask` or `handoff` with evidence, never through silent
  scope creep.";

const SUPERVISOR_BODY: &str = "\
## role-supervisor

- Observe the room for the Human: route directives to the Lead, watch for anti-patterns and
  loss of momentum, and receive batched Attention Letters.
- Protect the single control plane: detect duplicate Leads and escalate to the Human instead
  of spawning replacements.
- Replace a Lead only at a verified quiet point past its context rotation mark, after a
  reviewed handoff.
- You never edit code, dispatch Peers directly, or own technical acceptance.";

const LEAD_BODY: &str = "\
## role-lead

- You are the durable technical owner of one Project: read the repository's root
  `WORKSPACE_PROTOCOL.md` when present and fall back to your built-in operating baseline
  when absent.
- Decompose work into bounded Assignments (outcome, Disposition, Write Scope, Gate, thinking
  effort) and dispatch them to Peers without pre-solving the implementation.
- Verify, don't trust: run Gates against Candidate commits, validate handoffs against live
  Git state (`baseCommit..Candidate`), and integrate accepted Candidates manually.
- Own Writer Leases: one writer in the checkout, or up to three disjoint worktree writers;
  never let concurrent edits dirty the working tree.";

/// The Peer body carries the WORKSPACE_PROTOCOL visibility rule by omission: no Peer
/// artifact may reference or contain the boundary document (`GLOSSARY.md` WORKSPACE_PROTOCOL),
/// so the body states the attention boundary without naming it.
const PEER_BODY: &str = "\
## role-peer

- You execute exactly one bounded Assignment under one Disposition (Engineer, Architect,
  Reviewer, or Scout) and form your own technical position from code and verification.
- Your MCP surface is strictly `ask` and `handoff`; you have zero orchestration tools and
  never spawn or direct other agents.
- Before handing back writable work: run the Assignment's Gate, commit in a clean tree, and
  stay inside your declared Write Scope.
- Repository-wide workflow policy is the Lead's concern alone; give 100% of your attention
  to your Assignment brief.";

/// The built-in role body for `role` (`None` when `role` is not a contract role).
pub fn builtin_role_body(role: &str) -> Option<&'static str> {
    match role {
        "supervisor" => Some(SUPERVISOR_BODY),
        "lead" => Some(LEAD_BODY),
        "peer" => Some(PEER_BODY),
        _ => None,
    }
}

/// Renders the full contract for `role`: title, the three shared layers, then the body
/// (an override replaces only the body — the shared layers are fixed).
pub fn render_role_contract(role: &str, body_override: Option<&str>) -> String {
    let title = match role {
        "supervisor" => "Supervisor",
        "lead" => "Lead",
        "peer" => "Peer",
        _ => return String::new(),
    };
    let body = body_override
        .filter(|b| !b.trim().is_empty())
        .map(|b| b.to_string())
        .unwrap_or_else(|| builtin_role_body(role).unwrap_or("").to_string());
    format!(
        "# Hotpot Role Contract — {title}\n\
         \n\
         This contract is additive: it rides on top of your vendor base system prompt and\n\
         never replaces it.\n\
         \n\
         {SHARED_AUTHORITY}\n\
         \n\
         {SHARED_SEAT_IDENTITY}\n\
         \n\
         {CHALLENGE_SIGNALS}\n\
         \n\
         {body}\n"
    )
}

/// The rendered contract set for all roles plus its short deterministic digest.
#[derive(Debug, Clone)]
pub struct RenderedContracts {
    /// `role -> rendered contract` (BTreeMap: canonical iteration order for the digest).
    pub by_role: BTreeMap<String, String>,
    /// Short deterministic digest (hex) of the rendered contracts.
    pub digest: String,
}

impl RenderedContracts {
    /// Renders all role contracts from the given `role -> body` overrides.
    pub fn render(body_overrides: &BTreeMap<String, String>) -> Self {
        let mut by_role = BTreeMap::new();
        for role in CONTRACT_ROLES {
            let body = body_overrides.get(*role).map(String::as_str);
            by_role.insert((*role).to_string(), render_role_contract(role, body));
        }
        let digest = contract_digest(&by_role);
        Self { by_role, digest }
    }

    /// Renders with body overrides loaded from `$HOTPOT_CONTRACTS_DIR/<role>.md`.
    /// A missing directory or file falls back to the built-in body for that role.
    pub fn from_env() -> Self {
        let mut overrides = BTreeMap::new();
        if let Some(dir) = env::var_os("HOTPOT_CONTRACTS_DIR").filter(|v| !v.is_empty()) {
            for role in CONTRACT_ROLES {
                let path = Path::new(&dir).join(format!("{role}.md"));
                if let Ok(body) = fs::read_to_string(&path)
                    && !body.trim().is_empty()
                {
                    overrides.insert((*role).to_string(), body);
                }
            }
        }
        Self::render(&overrides)
    }

    /// The rendered contract for `role`.
    pub fn for_role(&self, role: &str) -> &str {
        self.by_role
            .get(role)
            .map(String::as_str)
            .unwrap_or_default()
    }
}

/// Short deterministic digest over the rendered contracts: length-framed per-role entries in
/// canonical (sorted) role order, hashed with SHA-256, truncated to
/// [`CONTRACT_DIGEST_HEX_CHARS`] hex characters. Same contract input → same digest; any
/// tweaked body → a different digest.
pub fn contract_digest(by_role: &BTreeMap<String, String>) -> String {
    let mut framed = String::new();
    for (role, contract) in by_role {
        framed.push_str(&format!("{role}:{}:{contract}\n", contract.len()));
    }
    let hex = sha256_hex(framed.as_bytes());
    hex[..CONTRACT_DIGEST_HEX_CHARS].to_string()
}

/// SHA-256 of `data` as lowercase hex (self-contained: the crate keeps a zero-network,
/// dependency-light build).
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// SHA-256 round constants (FIPS 180-4).
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

/// Pure SHA-256 implementation (FIPS 180-4); verified against known answer vectors below.
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Message padding: 0x80, zeros to 56 mod 64, then the big-endian bit length.
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for block in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (state, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *state = state.wrapping_add(value);
        }
    }

    let mut out = [0u8; 32];
    for (chunk, state) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&state.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known answer vectors (FIPS 180-4 / NIST examples) — the independent source of truth
    /// guarding the self-contained SHA-256 implementation.
    #[test]
    fn sha256_matches_known_answer_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}

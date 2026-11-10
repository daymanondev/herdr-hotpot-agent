//! Deterministic secret masking for gate tails and export bundles (spec:
//! "Secret Masking on Gate Tails & Exports").
//!
//! High-confidence secret patterns — `sk-`-style API keys, well-known token
//! prefixes (`ghp_`, `github_pat_`, Slack `xox?-`), AWS access key ids, and PEM
//! private-key blocks — are replaced by deterministic
//! `{{SECRET_<kind>_<hash>}}` placeholders. The hash is the first
//! [`HASH_LEN`] hex characters of the SHA-1 digest of the matched bytes, so the
//! same secret always maps to the same placeholder: two runs of the same
//! failing gate produce byte-identical tails, and a secret leaked into both a
//! gate tail and a ledger event collapses to the same placeholder in an export.
//!
//! The runtime takes no extra crates, so the scanner is hand-rolled (ASCII
//! patterns only; multibyte UTF-8 passes through untouched because no pattern
//! can match inside a multibyte sequence).

use super::ledger::sha1_hex;

/// Prefix every placeholder starts with (asserted by seam tests).
pub const SECRET_PLACEHOLDER_PREFIX: &str = "{{SECRET_";
/// Hex characters of the secret digest embedded in a placeholder.
const HASH_LEN: usize = 12;
/// Minimum characters after the `sk-` prefix before a match counts as an API key.
const API_KEY_MIN_TAIL: usize = 16;

/// Replaces every high-confidence secret in `input` with its deterministic
/// `{{SECRET_<kind>_<hash>}}` placeholder.
pub fn mask_text(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some((end, kind)) = match_secret_at(bytes, i) {
            out.extend_from_slice(placeholder(kind, &bytes[i..end]).as_bytes());
            i = end;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Patterns only ever match ASCII spans, so multibyte sequences are copied
    // verbatim and the output stays valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

/// The placeholder for one matched secret: `{{SECRET_<kind>_<sha1[0..12]>}}`.
fn placeholder(kind: &str, secret: &[u8]) -> String {
    // `{{`/`}}` are literal braces in format strings: doubled twice for the
    // two surrounding braces of the placeholder itself.
    format!(
        "{{{{SECRET_{kind}_{}}}}}",
        &sha1_hex(secret)[..HASH_LEN]
    )
}

/// Tries every high-confidence pattern anchored at `i`; returns the match end
/// and its kind. The PEM block matcher runs first so the longest, most
/// structured pattern wins.
fn match_secret_at(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match_private_key(bytes, i).or_else(|| match_token(bytes, i))
}

/// A PEM private-key block (`-----BEGIN ... PRIVATE KEY-----` through the full
/// `-----END ... PRIVATE KEY-----` line, markers included).
fn match_private_key(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    let rest = &bytes[i..];
    if !rest.starts_with(b"-----BEGIN ") {
        return None;
    }
    let line_end = rest.iter().position(|&b| b == b'\n')? + i;
    if !bytes[i..line_end].ends_with(b"PRIVATE KEY-----") {
        return None;
    }
    let mut search = line_end;
    while let Some(offset) = find_from(bytes, search, b"-----END ") {
        let eol = bytes[offset..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |p| offset + p);
        if bytes[offset..eol].ends_with(b"PRIVATE KEY-----") {
            // Swallow the END line's trailing newline so the placeholder
            // replaces the whole block cleanly.
            let end = (eol + 1).min(bytes.len());
            return Some((end, "private_key"));
        }
        search = offset + b"-----END ".len();
    }
    None
}

/// Prefix-style secrets: `sk-` API keys, `ghp_`-family and `github_pat_`
/// tokens, Slack `xox?-` tokens, and AWS `AKIA` access key ids.
fn match_token(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    let rest = &bytes[i..];
    if rest.starts_with(b"sk-") {
        let end = span_while(rest, 3, is_word_char);
        return (end - 3 >= API_KEY_MIN_TAIL).then_some((i + end, "api_key"));
    }
    for prefix in [b"ghp_", b"gho_", b"ghu_", b"ghs_", b"ghr_"] {
        if rest.starts_with(prefix) {
            let end = span_while(rest, prefix.len(), |b| b.is_ascii_alphanumeric());
            return (end - prefix.len() >= 20).then_some((i + end, "token"));
        }
    }
    if rest.starts_with(b"github_pat_") {
        let end = span_while(rest, 11, |b| b.is_ascii_alphanumeric() || b == b'_');
        return (end - 11 >= 30).then_some((i + end, "token"));
    }
    if rest.starts_with(b"xox")
        && matches!(rest.get(3), Some(b'b' | b'a' | b'p' | b'r' | b's'))
        && rest.get(4) == Some(&b'-')
    {
        let end = span_while(rest, 5, |b| b.is_ascii_alphanumeric() || b == b'-');
        return (end - 5 >= 10).then_some((i + end, "token"));
    }
    if rest.starts_with(b"AKIA") {
        // An AWS access key id is exactly AKIA + 16 upper-case alphanumerics;
        // a longer run is some other blob and stays untouched.
        let end = span_while(rest, 4, |b| b.is_ascii_uppercase() || b.is_ascii_digit());
        return (end - 4 == 16).then_some((i + end, "credential"));
    }
    None
}

/// Length of the run of `pred`-passing bytes starting at `from` in `bytes`.
fn span_while(bytes: &[u8], from: usize, pred: impl Fn(u8) -> bool) -> usize {
    let mut end = from;
    while end < bytes.len() && pred(bytes[end]) {
        end += 1;
    }
    end
}

/// Position of `needle` at or after `from`.
fn find_from(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    bytes[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

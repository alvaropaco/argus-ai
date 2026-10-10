//! Deterministic, fail-safe redaction for ledger uploads (spec 007 FR-004).
//!
//! The local ledger keeps full fidelity; the cloud never receives raw
//! arguments or secrets. The redactor is:
//!
//! - **deterministic** — the same input always yields the same output, so a
//!   retried upload is byte-identical and dedupable;
//! - **fail-safe** — the functions are total (they cannot panic or error), and
//!   the payload builders treat any construction failure as "drop the field",
//!   never "upload raw";
//! - **pattern-driven** — secret-named keys, `KEY=value` environment shapes,
//!   `Authorization:` header shapes, and high-entropy runs.

use serde_json::Value;

/// The placeholder every redacted value collapses to.
pub const REDACTED: &str = "[redacted]";

/// Key substrings that mark a field as secret-shaped (case-insensitive).
const SECRET_KEY_MARKERS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "authorization",
    "credential",
    "api_key",
    "apikey",
    "private_key",
    "privatekey",
    "cookie",
    "session_key",
    "bearer",
];

/// Minimum length of a candidate high-entropy run.
const ENTROPY_RUN_MIN: usize = 24;
/// Shannon-entropy floor (bits per char) for a run to be treated as a secret.
/// Hex-only material (ids, digests) tops out at 4.0, so it stays legible;
/// base64-shaped credentials sit well above.
const ENTROPY_FLOOR: f64 = 4.25;

/// Whether a key name marks its value as a secret.
pub fn is_secret_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SECRET_KEY_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

/// Redacts one JSON value in place-by-copy: secret-named object fields and
/// secret-shaped strings collapse to [`REDACTED`]; structure is preserved.
pub fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, inner)| {
                    if is_secret_key(key) {
                        (key.clone(), Value::String(REDACTED.to_string()))
                    } else {
                        (key.clone(), redact_value(inner))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::String(text) => Value::String(redact_text(text)),
        other => other.clone(),
    }
}

/// Redacts one string: authorization headers first (the precise shape), then
/// environment assignments, then high-entropy runs.
pub fn redact_text(text: &str) -> String {
    let auth = redact_authorization_headers(text);
    let out = redact_env_assignments(&auth);
    redact_high_entropy_runs(&out)
}

/// Replaces `SECRET=value` / `SECRET: value` pairs whose key is secret-shaped
/// (e.g. `TOKEN=xyz`, `api_key: xyz`) anywhere in the text. The whole value
/// gives way — fail-safe beats surgical — except a value the authorization
/// pass already redacted, which is left untouched.
fn redact_env_assignments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        let split = trimmed
            .find(['=', ':'])
            .map(|at| (at, trimmed.as_bytes()[at]));
        let Some((at, sep)) = split else {
            out.push_str(line);
            continue;
        };
        let key = trimmed[..at].trim();
        let value = trimmed[at + 1..].trim();
        if !key.is_empty() && !key.contains(' ') && is_secret_key(key) && !value.is_empty() {
            if value.starts_with(REDACTED) {
                // Already handled by the header pass; do not over-redact the
                // remainder of the line.
                out.push_str(line);
                continue;
            }
            let prefix = &line[..line.len() - trimmed.len()];
            out.push_str(prefix);
            out.push_str(key);
            out.push(if sep == b'=' { '=' } else { ':' });
            out.push_str(REDACTED);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Replaces `Authorization: …` shapes to end of line — the whole header value
/// is sensitive (scheme included), so nothing after the colon survives.
fn redact_authorization_headers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.to_ascii_lowercase().find("authorization") {
        let before = &rest[..at];
        let after = &rest[at..];
        let Some(colon) = after.find(':') else {
            out.push_str(before);
            out.push_str(after);
            rest = "";
            break;
        };
        let value = &after[colon + 1..];
        let line_end = value
            .find('\n')
            .map(|at| colon + 1 + at)
            .unwrap_or(after.len());
        // An empty header value is not a secret; anything else redacts whole.
        if value.trim().is_empty() {
            out.push_str(before);
            out.push_str(after);
            rest = "";
            break;
        }
        out.push_str(before);
        out.push_str(&after[..colon]);
        out.push_str(": ");
        out.push_str(REDACTED);
        // Preserve the newline that ended this header's line, if any.
        if line_end < after.len() {
            out.push('\n');
            rest = &after[line_end + 1..];
        } else {
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// Replaces runs of credential-shaped characters that carry more entropy than
/// honest identifiers do (FR-004: "high-entropy strings").
fn redact_high_entropy_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= ENTROPY_RUN_MIN && shannon_entropy(run) >= ENTROPY_FLOOR {
            out.push_str(REDACTED);
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '_' | '-' | '.') {
            run.push(ch);
        } else {
            flush(&mut run, &mut out);
            out.push(ch);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Shannon entropy in bits per character over the byte view.
fn shannon_entropy(text: &str) -> f64 {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for byte in bytes {
        counts[*byte as usize] += 1;
    }
    let len = bytes.len() as f64;
    counts
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let p = *count as f64 / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_named_keys_collapse_at_every_depth() {
        let value = json!({
            "unit": "nginx.service",
            "api_key": "sk-abc123",
            "nested": {
                "PASSWORD": "hunter2",
                "deep": [{ "session_key": "xyz" }]
            }
        });
        let redacted = redact_value(&value);
        assert_eq!(redacted["unit"], "nginx.service", "honest fields survive");
        assert_eq!(redacted["api_key"], REDACTED);
        assert_eq!(redacted["nested"]["PASSWORD"], REDACTED);
        assert_eq!(redacted["nested"]["deep"][0]["session_key"], REDACTED);
    }

    #[test]
    fn the_redaction_is_deterministic() {
        let value = json!({ "args": "TOKEN=xyz restart", "id": "abc" });
        assert_eq!(redact_value(&value), redact_value(&value));
    }

    #[test]
    fn environment_shapes_are_redacted() {
        assert_eq!(redact_text("TOKEN=xyz"), format!("TOKEN={REDACTED}"));
        // The whole value gives way: fail-safe beats surgical.
        assert_eq!(
            redact_text("DEPLOY_TOKEN=abc123 plus tail"),
            format!("DEPLOY_TOKEN={REDACTED}")
        );
        assert!(
            redact_text("UNIT=nginx").contains("nginx"),
            "non-secret keys stay"
        );
    }

    #[test]
    fn authorization_headers_are_redacted_to_end_of_line() {
        assert_eq!(
            redact_text("Authorization: Bearer sk-live-abc123 rest"),
            format!("Authorization: {REDACTED}")
        );
        assert_eq!(
            redact_text("authorization:Basic dXNlcjpwYXNz"),
            format!("authorization: {REDACTED}")
        );
        // A following line is untouched.
        assert_eq!(
            redact_text("Authorization: Bearer x\nunit: nginx"),
            format!("Authorization: {REDACTED}\nunit: nginx")
        );
    }

    #[test]
    fn high_entropy_runs_are_redacted_but_identifiers_survive() {
        // A base64-shaped credential: long, mixed-class, high entropy.
        let secret = "j8Kp2mQ7vX4nR1sT9wZ3bY6uF0hL5dG8aCeI";
        let redacted = redact_text(concat!(
            "curl -H X-Key:",
            "j8Kp2mQ7vX4nR1sT9wZ3bY6uF0hL5dG8aCeI"
        ));
        assert!(redacted.contains(REDACTED), "{redacted}");
        assert!(
            !redacted.contains(secret),
            "the raw secret must not survive"
        );

        // UUIDs and unit names are legible identifiers, not secrets.
        let id_line = "correlation 3f2b8c9e-1a4d-4e5f-9a8b-7c6d5e4f3a2b unit nginx.service";
        assert_eq!(
            redact_text(id_line),
            id_line,
            "hex identifiers stay below the entropy floor"
        );
    }

    #[test]
    fn structure_is_preserved_and_the_function_is_total() {
        let value = json!({ "a": [1, 2.5, null, true, "text"], "b": {} });
        let redacted = redact_value(&value);
        assert_eq!(redacted["a"][0], 1);
        assert_eq!(redacted["a"][3], true);
        assert_eq!(redacted["b"], json!({}));
    }
}

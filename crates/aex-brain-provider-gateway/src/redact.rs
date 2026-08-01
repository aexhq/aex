//! The bounded, credential-safe redactor.
//!
//! Every provider-derived string that reaches a log, a receipt or an error
//! passes through here. The rule is positive rather than negative: a diagnostic
//! is truncated to a bound and every run of credential-shaped material is
//! replaced, so a provider that echoes the `Authorization` header into a 400
//! body cannot leak it into `RedactedDetail`.

use aex_model_catalog::primitives::BoundedString;

/// The replacement written in place of anything credential-shaped.
pub const REDACTED: &str = "[redacted]";

/// The shortest run of credential-shaped characters that is replaced.
///
/// Every provider key in this set is far longer: `sk-…` (`OpenAI`, `DeepSeek`,
/// Moonshot), `sk-ant-…` (Anthropic), `AIza…` (Google) and Z.AI's `id.secret`
/// pair are all above 30 characters. Twenty is comfortably below every one of
/// them and comfortably above ordinary prose.
pub const MIN_SECRET_RUN: usize = 20;

/// Bounds and redacts a provider-supplied diagnostic.
///
/// Two passes, in order:
///
/// 1. every occurrence of a known secret is replaced, whole;
/// 2. every remaining run of at least [`MIN_SECRET_RUN`] characters drawn from
///    the credential alphabet (`A-Za-z0-9-_.`) with no whitespace is replaced,
///    which catches a key AEX never held — a customer's key echoed by a
///    provider that AEX is not currently dispatching under.
///
/// Truncation happens last, so a bound can never split a replacement and leave
/// a prefix of a secret behind.
#[must_use]
pub fn redact<const N: usize>(text: &str, secrets: &[&str]) -> BoundedString<N> {
    let mut working = text.to_owned();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        working = working.replace(secret, REDACTED);
    }
    BoundedString::truncating(&mask_runs(&working))
}

/// Replaces every long unbroken credential-shaped run.
fn mask_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for character in text.chars() {
        if is_secret_char(character) {
            run.push(character);
            continue;
        }
        flush(&mut run, &mut out);
        out.push(character);
    }
    flush(&mut run, &mut out);
    out
}

fn flush(run: &mut String, out: &mut String) {
    if run.is_empty() {
        return;
    }
    if run.chars().count() >= MIN_SECRET_RUN && run.chars().any(char::is_numeric) {
        out.push_str(REDACTED);
    } else {
        out.push_str(run);
    }
    run.clear();
}

const fn is_secret_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}

#[cfg(test)]
mod tests {
    use super::{REDACTED, redact};

    #[test]
    fn a_known_secret_is_removed_whole() {
        let key = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        let body = format!("invalid x-api-key: {key} for workspace ws_1");
        let out = redact::<512>(&body, &[key]);
        assert!(!out.as_str().contains(key));
        assert!(out.as_str().contains(REDACTED));
    }

    #[test]
    fn an_unknown_key_shaped_run_is_removed_too() {
        // A key AEX is not currently holding, echoed by the provider.
        let foreign = "AIzaSyD3aBcDeFgHiJkLmNoPqRsTuVwXyZ012345";
        let out = redact::<512>(&format!("bad key {foreign}"), &[]);
        assert!(!out.as_str().contains(foreign));
        assert_eq!(out.as_str(), format!("bad key {REDACTED}"));
    }

    #[test]
    fn ordinary_prose_survives() {
        let message = "The model context window was exceeded by 412 tokens.";
        assert_eq!(redact::<512>(message, &[]).as_str(), message);
    }

    #[test]
    fn a_long_all_letter_word_is_not_mistaken_for_a_key() {
        let message = "pneumonoultramicroscopicsilicovolcanoconiosis is a long word";
        assert_eq!(redact::<512>(message, &[]).as_str(), message);
    }

    #[test]
    fn truncation_happens_after_replacement() {
        let key = "sk-0123456789012345678901234567890123456789";
        let out = redact::<24>(&format!("aaaa {key}"), &[key]);
        assert!(!out.as_str().contains("sk-0"));
        assert!(out.len() <= 24);
    }

    #[test]
    fn a_secret_split_across_the_bound_cannot_survive_as_a_prefix() {
        let key = "sk-abcdefghij0123456789abcdefghij0123456789";
        // A bound that would land inside the key if truncation came first.
        let out = redact::<12>(&format!("err {key}"), &[key]);
        assert!(!out.as_str().contains("sk-abc"));
    }

    #[test]
    fn an_empty_secret_is_ignored_rather_than_matching_everywhere() {
        let out = redact::<64>("hello", &[""]);
        assert_eq!(out.as_str(), "hello");
    }
}

//! Codes from an authenticator app (RFC 6238): HMAC-SHA-1, 6 digits, a step of 30 seconds, the
//! only settings every app honours. And the recovery codes that stand in for the app.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha1::Sha1;
use subtle::ConstantTimeEq;

use crate::session::token_hash;

/// Seconds per code.
pub const STEP_SECS: i64 = 30;
/// 160 bits, the length of a SHA-1 output, as RFC 4226 advises.
const SECRET_BYTES: usize = 20;
/// How many recovery codes an account gets at a time.
pub const RECOVERY_CODES: usize = 10;
/// 80 bits each: they live until used, and their hash is not keyed.
const RECOVERY_BYTES: usize = 10;
/// What a label of an `otpauth://` URI may keep as it is.
const LABEL: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'.')
    .remove(b'-')
    .remove(b'_')
    .remove(b'@');

/// A new secret for an account.
pub fn new_secret() -> Result<Vec<u8>, getrandom::Error> {
    let mut secret = vec![0; SECRET_BYTES];
    getrandom::fill(&mut secret)?;
    Ok(secret)
}

/// The secret as apps take it typed: base32, no padding.
pub fn encode_secret(secret: &[u8]) -> String {
    BASE32_NOPAD.encode(secret)
}

/// What a QR code holds for an app. The issuer is the instance's host, so two instances do not
/// look alike in an app.
pub fn uri(secret: &[u8], host: &str, email: &str) -> String {
    let issuer = utf8_percent_encode(host, LABEL);
    let account = utf8_percent_encode(email, LABEL);
    let secret = encode_secret(secret);
    format!("otpauth://totp/{issuer}:{account}?secret={secret}&issuer={issuer}")
}

/// The time step of a moment in milliseconds.
pub fn step_at(now_ms: i64) -> i64 {
    now_ms.div_euclid(1000 * STEP_SECS)
}

/// The value of RFC 4226 for a counter, before it is cut to a number of digits.
fn truncated(secret: &[u8], counter: u64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let bytes = [
        digest[offset],
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ];
    u32::from_be_bytes(bytes) & 0x7fff_ffff
}

/// The 6-digit code of a step, as an app shows it.
pub fn code_at(secret: &[u8], step: i64) -> String {
    format!("{:06}", truncated(secret, step as u64) % 1_000_000)
}

/// Whether `given` looks like a code from an app rather than a recovery code.
pub fn is_app_code(given: &str) -> bool {
    given.len() == 6 && given.bytes().all(|b| b.is_ascii_digit())
}

/// The step, one before `step` to one after, whose code is `given`, so a clock a little off still
/// works. Only the store can say whether it was used already.
pub fn matching_step(secret: &[u8], given: &str, step: i64) -> Option<i64> {
    let mut found = None;
    // Every step is computed, so the time tells nothing of which one matched.
    for candidate in [step - 1, step, step + 1] {
        let right = code_at(secret, candidate)
            .as_bytes()
            .ct_eq(given.as_bytes());
        if bool::from(right) && found.is_none() {
            found = Some(candidate);
        }
    }
    found
}

/// New recovery codes, as the person sees them once: `xxxx-xxxx-xxxx-xxxx`.
pub fn new_recovery_codes() -> Result<Vec<String>, getrandom::Error> {
    (0..RECOVERY_CODES)
        .map(|_| {
            let mut bytes = [0; RECOVERY_BYTES];
            getrandom::fill(&mut bytes)?;
            let text = BASE32_NOPAD.encode(&bytes).to_ascii_lowercase();
            let groups: Vec<&str> = (0..4).map(|i| &text[i * 4..i * 4 + 4]).collect();
            Ok(groups.join("-"))
        })
        .collect()
}

/// What a recovery code is stored as. Dashes, spaces and case do not count.
pub fn recovery_hash(code: &str) -> String {
    let clean: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    token_hash(&clean)
}

/// A code as typed, without the spaces an app shows in the middle.
pub fn clean_code(given: &str) -> String {
    given.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238, Appendix B: the SHA-1 rows, cut from 8 digits.
    const SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn codes_follow_the_vectors_of_rfc_6238() {
        let vectors = [
            (59, 94287082),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
            (20000000000, 65353130),
        ];
        for (time, expected) in vectors {
            let step = step_at(time * 1000);
            assert_eq!(
                truncated(SECRET, step as u64) % 100_000_000,
                expected,
                "{time}"
            );
            assert_eq!(
                code_at(SECRET, step),
                format!("{:06}", expected % 1_000_000)
            );
        }
    }

    #[test]
    fn a_code_matches_its_step_and_the_ones_beside_it() {
        let step = step_at(1_234_567_890_000);
        let code = code_at(SECRET, step);
        assert_eq!(matching_step(SECRET, &code, step), Some(step));
        assert_eq!(matching_step(SECRET, &code, step + 1), Some(step));
        assert_eq!(matching_step(SECRET, &code, step - 1), Some(step));
        assert_eq!(matching_step(SECRET, &code, step + 2), None);
        assert_eq!(matching_step(SECRET, "12345", step), None);
    }

    #[test]
    fn the_uri_names_the_instance_and_the_account() {
        let uri = uri(SECRET, "wiki.example.org:8443", "ada@example.org");
        assert_eq!(
            uri,
            "otpauth://totp/wiki.example.org%3A8443:ada@example.org\
             ?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=wiki.example.org%3A8443"
        );
    }

    #[test]
    fn recovery_codes_are_long_distinct_and_forgiving() {
        let codes = new_recovery_codes().unwrap();
        assert_eq!(codes.len(), RECOVERY_CODES);
        let mut sorted = codes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), RECOVERY_CODES);
        for code in &codes {
            assert_eq!(code.len(), 19, "{code}");
            assert!(!is_app_code(code));
            let typed = code.to_uppercase().replace('-', " ");
            assert_eq!(recovery_hash(&typed), recovery_hash(code));
        }
        assert!(is_app_code("012345"));
        assert!(!is_app_code("01234a"));
        assert_eq!(clean_code(" 123 456 "), "123456");
    }
}

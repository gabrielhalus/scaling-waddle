use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const TOLERANCE_SECS: i64 = 300;

#[derive(Debug, PartialEq, Eq)]
pub enum Rejected {
    Missing,
    Malformed,
    Expired,
    Mismatch,
}

fn mac(secret: &str, timestamp: i64, body: &[u8]) -> Option<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Some(mac)
}

#[cfg(test)]
pub fn sign(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mac = mac(secret, timestamp, body).unwrap();
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

pub fn verify(
    secret: &str,
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
    now_secs: i64,
) -> Result<(), Rejected> {
    let (Some(timestamp), Some(signature)) = (timestamp, signature) else {
        return Err(Rejected::Missing);
    };
    let timestamp: i64 = timestamp.trim().parse().map_err(|_| Rejected::Malformed)?;
    let provided = signature
        .strip_prefix("sha256=")
        .and_then(|digest| hex::decode(digest).ok())
        .ok_or(Rejected::Malformed)?;
    if (now_secs - timestamp).abs() > TOLERANCE_SECS {
        return Err(Rejected::Expired);
    }
    mac(secret, timestamp, body)
        .ok_or(Rejected::Mismatch)?
        .verify_slice(&provided)
        .map_err(|_| Rejected::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_valid_signature_is_accepted() {
        let sig = sign("secret", NOW, b"{\"a\":1}");
        let ts = NOW.to_string();
        assert_eq!(
            verify("secret", Some(&ts), Some(&sig), b"{\"a\":1}", NOW),
            Ok(())
        );
    }

    #[test]
    fn a_tampered_body_or_wrong_secret_is_rejected() {
        let sig = sign("secret", NOW, b"body");
        let ts = NOW.to_string();
        assert_eq!(
            verify("secret", Some(&ts), Some(&sig), b"autre", NOW),
            Err(Rejected::Mismatch)
        );
        assert_eq!(
            verify("autre", Some(&ts), Some(&sig), b"body", NOW),
            Err(Rejected::Mismatch)
        );
    }

    #[test]
    fn the_timestamp_is_part_of_the_signature() {
        let sig = sign("secret", NOW, b"body");
        let ts = (NOW + 1).to_string();
        assert_eq!(
            verify("secret", Some(&ts), Some(&sig), b"body", NOW),
            Err(Rejected::Mismatch)
        );
    }

    #[test]
    fn old_and_future_timestamps_are_rejected_as_replays() {
        for ts in [NOW - TOLERANCE_SECS - 1, NOW + TOLERANCE_SECS + 1] {
            let sig = sign("secret", ts, b"body");
            assert_eq!(
                verify("secret", Some(&ts.to_string()), Some(&sig), b"body", NOW),
                Err(Rejected::Expired)
            );
        }
        let edge = NOW - TOLERANCE_SECS;
        let sig = sign("secret", edge, b"body");
        assert_eq!(
            verify("secret", Some(&edge.to_string()), Some(&sig), b"body", NOW),
            Ok(())
        );
    }

    #[test]
    fn missing_or_malformed_headers_are_rejected() {
        let sig = sign("secret", NOW, b"body");
        let ts = NOW.to_string();
        assert_eq!(
            verify("secret", None, Some(&sig), b"body", NOW),
            Err(Rejected::Missing)
        );
        assert_eq!(
            verify("secret", Some(&ts), None, b"body", NOW),
            Err(Rejected::Missing)
        );
        assert_eq!(
            verify("secret", Some("x"), Some(&sig), b"body", NOW),
            Err(Rejected::Malformed)
        );
        assert_eq!(
            verify("secret", Some(&ts), Some("abc"), b"body", NOW),
            Err(Rejected::Malformed)
        );
        assert_eq!(
            verify("secret", Some(&ts), Some("sha256=zz"), b"body", NOW),
            Err(Rejected::Malformed)
        );
    }
}

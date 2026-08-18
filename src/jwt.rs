//! JWT credentials: decode one to see what you are actually sending, or sign a
//! fresh HS256 one from claims + secret for a dev server.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::model::{base64_url, base64_url_decode, now_unix};

pub struct Decoded {
    pub header: String,
    pub payload: String,
    pub alg: String,
    /// `exp` claim, if present.
    pub expires_at: Option<i64>,
    pub subject: Option<String>,
    pub issuer: Option<String>,
}

impl Decoded {
    pub fn seconds_left(&self) -> Option<i64> {
        self.expires_at.map(|exp| exp - now_unix() as i64)
    }

    pub fn expired(&self) -> bool {
        self.seconds_left().is_some_and(|s| s <= 0)
    }
}

/// Split and base64url-decode a token. The signature is not verified - this is
/// for seeing the claims, not for trusting them.
pub fn decode(token: &str) -> Result<Decoded, String> {
    let token = token.trim();
    let mut parts = token.split('.');
    let (Some(h), Some(p)) = (parts.next(), parts.next()) else {
        return Err("not a jwt: expected header.payload.signature".to_owned());
    };
    if h.is_empty() || p.is_empty() {
        return Err("not a jwt: empty header or payload".to_owned());
    }

    let header = pretty(&decode_segment(h, "header")?);
    let payload_raw = decode_segment(p, "payload")?;
    let payload = pretty(&payload_raw);

    let claims: serde_json::Value = serde_json::from_str(&payload_raw).unwrap_or_default();
    let head: serde_json::Value = serde_json::from_str(&header).unwrap_or_default();

    Ok(Decoded {
        alg: head
            .get("alg")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_owned(),
        expires_at: claims.get("exp").and_then(|v| v.as_i64()),
        subject: claims
            .get("sub")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        issuer: claims
            .get("iss")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        header,
        payload,
    })
}

fn decode_segment(segment: &str, what: &str) -> Result<String, String> {
    let bytes = base64_url_decode(segment).map_err(|e| format!("bad {what}: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("bad {what}: {e}"))
}

fn pretty(json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| json.to_owned())
}

/// Sign `claims` with HS256. `secret_is_base64` treats the secret as base64url
/// (or base64) encoded key material rather than raw bytes.
pub fn sign_hs256(claims: &str, secret: &str, secret_is_base64: bool) -> Result<String, String> {
    let claims_value: serde_json::Value =
        serde_json::from_str(claims).map_err(|e| format!("claims are not json: {e}"))?;
    let claims_json =
        serde_json::to_string(&claims_value).map_err(|e| format!("claims: {e}"))?;

    let key = if secret_is_base64 {
        base64_url_decode(secret).map_err(|e| format!("secret: {e}"))?
    } else {
        secret.as_bytes().to_vec()
    };
    if key.is_empty() {
        return Err("secret is empty".to_owned());
    }

    let header = base64_url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = base64_url(claims_json.as_bytes());
    let signing_input = format!("{header}.{payload}");

    let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|e| e.to_string())?;
    mac.update(signing_input.as_bytes());
    let sig = base64_url(&mac.finalize().into_bytes());

    Ok(format!("{signing_input}.{sig}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // jwt.io reference token: HS256 over {"sub":"1234567890","name":"John Doe","iat":1516239022}
    // with the secret "your-256-bit-secret"
    const REFERENCE: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

    #[test]
    fn decodes_a_known_token() {
        let d = decode(REFERENCE).unwrap();
        assert_eq!(d.alg, "HS256");
        assert_eq!(d.subject.as_deref(), Some("1234567890"));
        assert!(d.payload.contains("John Doe"));
        assert_eq!(d.expires_at, None);
        assert!(!d.expired(), "no exp claim means no expiry");
    }

    #[test]
    fn signing_matches_the_reference_token() {
        let claims = r#"{"sub":"1234567890","name":"John Doe","iat":1516239022}"#;
        let signed = sign_hs256(claims, "your-256-bit-secret", false).unwrap();
        assert_eq!(signed, REFERENCE);
    }

    #[test]
    fn round_trips_what_it_signs() {
        let signed = sign_hs256(r#"{"sub":"me","exp":1}"#, "k", false).unwrap();
        let d = decode(&signed).unwrap();
        assert_eq!(d.subject.as_deref(), Some("me"));
        assert_eq!(d.expires_at, Some(1));
        assert!(d.expired());
    }

    #[test]
    fn rejects_junk() {
        assert!(decode("nope").is_err());
        assert!(decode("").is_err());
        assert!(sign_hs256("not json", "k", false).is_err());
        assert!(sign_hs256("{}", "", false).is_err());
    }
}

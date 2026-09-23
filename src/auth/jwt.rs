use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessTokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub iat: u64,
    pub client_id: String,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<u64>,
}

pub fn issue(secret: &str, claims: &AccessTokenClaims) -> Result<String, String> {
    let header = json!({"alg": "HS256", "typ": "JWT"});
    let header = serde_json::to_vec(&header).map_err(|error| error.to_string())?;
    let payload = serde_json::to_vec(claims).map_err(|error| error.to_string())?;

    let encoded_header = URL_SAFE_NO_PAD.encode(header);
    let encoded_payload = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{encoded_header}.{encoded_payload}");

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|error| error.to_string())?;
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();

    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

pub fn verify(
    secret: &str,
    token: &str,
    expected_issuer: &str,
    expected_audience: &str,
    now: u64,
) -> Result<AccessTokenClaims, String> {
    let mut parts = token.split('.');
    let Some(encoded_header) = parts.next() else {
        return Err("malformed JWT".to_string());
    };
    let Some(encoded_payload) = parts.next() else {
        return Err("malformed JWT".to_string());
    };
    let Some(encoded_signature) = parts.next() else {
        return Err("malformed JWT".to_string());
    };
    if parts.next().is_some() {
        return Err("malformed JWT".to_string());
    }

    let header = URL_SAFE_NO_PAD
        .decode(encoded_header)
        .map_err(|_| "invalid JWT header encoding".to_string())?;
    let header: serde_json::Value =
        serde_json::from_slice(&header).map_err(|_| "invalid JWT header".to_string())?;
    if header.get("alg").and_then(|value| value.as_str()) != Some("HS256") {
        return Err("unsupported JWT algorithm".to_string());
    }

    let signature = URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| "invalid JWT signature encoding".to_string())?;
    let signing_input = format!("{encoded_header}.{encoded_payload}");
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|error| error.to_string())?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| "invalid JWT signature".to_string())?;

    let payload = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .map_err(|_| "invalid JWT payload encoding".to_string())?;
    let claims: AccessTokenClaims =
        serde_json::from_slice(&payload).map_err(|_| "invalid JWT claims".to_string())?;

    if claims.iss != expected_issuer {
        return Err("token issuer mismatch".to_string());
    }
    if claims.aud != expected_audience {
        return Err("token audience mismatch".to_string());
    }
    if claims.iat > now.saturating_add(60) {
        return Err("token issued-at time is in the future".to_string());
    }
    if claims.exp.is_some_and(|exp| exp <= now) {
        return Err("token expired".to_string());
    }
    if claims.sub.trim().is_empty() || claims.client_id.trim().is_empty() {
        return Err("token identity claims are missing".to_string());
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_and_verifies_hs256_token() {
        let claims = AccessTokenClaims {
            iss: "https://issuer.example".to_string(),
            sub: "local-user".to_string(),
            aud: "https://mcp.example".to_string(),
            iat: 100,
            client_id: "client".to_string(),
            scope: "forge:read".to_string(),
            exp: Some(200),
        };

        let token = issue("01234567890123456789012345678901", &claims).unwrap();
        let verified = verify(
            "01234567890123456789012345678901",
            &token,
            "https://issuer.example",
            "https://mcp.example",
            150,
        )
        .unwrap();

        assert_eq!(verified.client_id, "client");
    }

    #[test]
    fn rejects_wrong_audience() {
        let claims = AccessTokenClaims {
            iss: "https://issuer.example".to_string(),
            sub: "local-user".to_string(),
            aud: "https://mcp.example".to_string(),
            iat: 100,
            client_id: "client".to_string(),
            scope: "forge:read".to_string(),
            exp: None,
        };

        let token = issue("01234567890123456789012345678901", &claims).unwrap();
        let error = verify(
            "01234567890123456789012345678901",
            &token,
            "https://issuer.example",
            "https://other.example",
            150,
        )
        .unwrap_err();

        assert!(error.contains("audience"));
    }
}

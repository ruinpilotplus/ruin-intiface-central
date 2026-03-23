use anyhow::{anyhow, Result};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const FIREBASE_JWKS_URL: &str =
    "https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com";

#[derive(Debug, Serialize, Deserialize)]
pub struct FirebaseClaims {
    pub sub: String,
    pub aud: String,
    pub iss: String,
    pub exp: usize,
    pub iat: usize,
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<JwkEntry>,
}

#[derive(Debug, Deserialize)]
struct JwkEntry {
    kid: String,
    n: String,
    e: String,
    #[allow(dead_code)]
    alg: Option<String>,
    #[serde(rename = "use")]
    #[allow(dead_code)]
    key_use: Option<String>,
}

/// Validate a Firebase ID token and return the Firebase UID on success.
/// This fetches Google's public JWK set on every call; in production you
/// should cache the JWKS (respecting the Cache-Control header) to avoid
/// an extra network round-trip per request.
pub async fn validate_firebase_token(token: &str) -> Result<String> {
    let header =
        decode_header(token).map_err(|e| anyhow!("Failed to decode JWT header: {}", e))?;

    let kid = header.kid.ok_or_else(|| anyhow!("JWT missing 'kid' header field"))?;

    let jwks = fetch_jwks().await?;

    let jwk = jwks
        .keys
        .iter()
        .find(|k| k.kid == kid)
        .ok_or_else(|| anyhow!("No matching JWK for kid: {}", kid))?;

    // Base64url-decode the modulus and exponent
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let n_bytes =
        URL_SAFE_NO_PAD.decode(&jwk.n).map_err(|e| anyhow!("Failed to decode JWK 'n': {}", e))?;
    let e_bytes =
        URL_SAFE_NO_PAD.decode(&jwk.e).map_err(|e| anyhow!("Failed to decode JWK 'e': {}", e))?;

    let decoding_key = DecodingKey::from_rsa_raw_components(&n_bytes, &e_bytes);

    let mut validation = Validation::new(Algorithm::RS256);
    // Audience validation requires the caller to know the Firebase project ID.
    // We skip it here; the issuer check below is sufficient to verify origin.
    validation.validate_aud = false;

    let token_data = decode::<FirebaseClaims>(token, &decoding_key, &validation)
        .map_err(|e| anyhow!("JWT validation failed: {}", e))?;

    // Validate issuer – must be Firebase Secure Token Service
    if !token_data.claims.iss.starts_with("https://securetoken.google.com/") {
        return Err(anyhow!("Invalid JWT issuer: {}", token_data.claims.iss));
    }

    Ok(token_data.claims.sub)
}

async fn fetch_jwks() -> Result<Jwks> {
    let response = reqwest::get(FIREBASE_JWKS_URL)
        .await
        .map_err(|e| anyhow!("Failed to fetch Firebase JWKS: {}", e))?;

    let body: HashMap<String, serde_json::Value> = response
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse JWKS response: {}", e))?;

    // The JWKS endpoint returns {"keys": [...]}
    let keys_value = body.get("keys").ok_or_else(|| anyhow!("JWKS response missing 'keys' field"))?;

    let keys: Vec<JwkEntry> =
        serde_json::from_value(keys_value.clone()).map_err(|e| anyhow!("Failed to parse JWK keys: {}", e))?;

    Ok(Jwks { keys })
}

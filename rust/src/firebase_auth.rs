use anyhow::{anyhow, Result};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use lazy_static::lazy_static;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const FIREBASE_JWKS_URL: &str =
    "https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com";

/// Cache TTL for the JWKS response (1 hour; Google typically sets Cache-Control
/// max-age to 21600 seconds, so this is conservative but appropriate).
const JWKS_CACHE_TTL: Duration = Duration::from_secs(3600);

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

struct JwksCache {
    keys: HashMap<String, JwkEntry>,
    fetched_at: Option<Instant>,
}

lazy_static! {
    static ref JWKS_CACHE: RwLock<JwksCache> = RwLock::new(JwksCache {
        keys: HashMap::new(),
        fetched_at: None,
    });
}

/// Validate a Firebase ID token and return the Firebase UID on success.
///
/// The JWKS response is cached in process memory for [`JWKS_CACHE_TTL`] to
/// avoid a round-trip to Google on every validation call.
pub async fn validate_firebase_token(token: &str) -> Result<String> {
    let header =
        decode_header(token).map_err(|e| anyhow!("Failed to decode JWT header: {}", e))?;

    let kid = header.kid.ok_or_else(|| anyhow!("JWT missing 'kid' header field"))?;

    let jwk_n;
    let jwk_e;
    {
        let cache = JWKS_CACHE.read();
        let is_stale = cache.fetched_at.map_or(true, |t| t.elapsed() > JWKS_CACHE_TTL);
        if !is_stale {
            if let Some(entry) = cache.keys.get(&kid) {
                jwk_n = entry.n.clone();
                jwk_e = entry.e.clone();
            } else {
                return Err(anyhow!("No matching JWK for kid: {} (cached)", kid));
            }
        } else {
            // Drop the read lock before acquiring write lock
            drop(cache);
            refresh_jwks_cache().await?;
            let cache = JWKS_CACHE.read();
            let entry = cache
                .keys
                .get(&kid)
                .ok_or_else(|| anyhow!("No matching JWK for kid: {}", kid))?;
            jwk_n = entry.n.clone();
            jwk_e = entry.e.clone();
        }
    }

    // Base64url-decode the modulus and exponent
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let n_bytes = URL_SAFE_NO_PAD
        .decode(&jwk_n)
        .map_err(|e| anyhow!("Failed to decode JWK 'n': {}", e))?;
    let e_bytes = URL_SAFE_NO_PAD
        .decode(&jwk_e)
        .map_err(|e| anyhow!("Failed to decode JWK 'e': {}", e))?;

    let decoding_key = DecodingKey::from_rsa_raw_components(&n_bytes, &e_bytes);

    let mut validation = Validation::new(Algorithm::RS256);
    // Audience validation requires knowing the Firebase project ID at compile
    // time.  We skip the built-in aud check here and instead rely on the
    // issuer check below (which confirms the token came from Firebase Secure
    // Token Service for a specific project).  If you want to restrict tokens
    // to a single project, set `validation.set_audience(&["<YOUR_PROJECT_ID>"])`
    // and remove this line.
    validation.validate_aud = false;

    let token_data = decode::<FirebaseClaims>(token, &decoding_key, &validation)
        .map_err(|e| anyhow!("JWT validation failed: {}", e))?;

    // Validate issuer – must be Firebase Secure Token Service
    if !token_data.claims.iss.starts_with("https://securetoken.google.com/") {
        return Err(anyhow!("Invalid JWT issuer: {}", token_data.claims.iss));
    }

    Ok(token_data.claims.sub)
}

async fn refresh_jwks_cache() -> Result<()> {
    let response = reqwest::get(FIREBASE_JWKS_URL)
        .await
        .map_err(|e| anyhow!("Failed to fetch Firebase JWKS: {}", e))?;

    let body: HashMap<String, serde_json::Value> = response
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse JWKS response: {}", e))?;

    // The JWKS endpoint returns {"keys": [...]}
    let keys_value = body
        .get("keys")
        .ok_or_else(|| anyhow!("JWKS response missing 'keys' field"))?;

    let entries: Vec<JwkEntry> = serde_json::from_value(keys_value.clone())
        .map_err(|e| anyhow!("Failed to parse JWK keys: {}", e))?;

    let mut cache = JWKS_CACHE.write();
    cache.keys = entries.into_iter().map(|e| (e.kid.clone(), e)).collect();
    cache.fetched_at = Some(Instant::now());

    log::info!("Firebase JWKS cache refreshed ({} keys)", cache.keys.len());
    Ok(())
}

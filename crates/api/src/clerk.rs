use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::FromRef;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::{Notify, RwLock};

use crate::routes::ErrorResponse;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct ClerkUser {
    pub user_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeyVeraAuthMode {
    Clerk,
    LocalDevelopment,
}

#[derive(Debug, Clone)]
pub struct HeyVeraAuthConfig {
    pub mode: HeyVeraAuthMode,
    pub clerk_secret_key: Option<String>,
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_flag(key: &str) -> bool {
    std::env::var(key)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// True when HeyVera must reject all local/dev authentication behavior.
pub(crate) fn is_production_runtime() -> bool {
    crate::is_production_env() || env_flag("HEYVERA_REQUIRE_AUTH")
}

/// Explicit headerless local auth is only valid outside production.
pub(crate) fn local_auth_allowed() -> bool {
    !is_production_runtime() && env_flag("CORTEX_AUTH_DISABLED")
}

fn valid_https_endpoint(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn valid_https_origin(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    valid_https_endpoint(value) && url.path() == "/"
}

fn validate_auth_config_values(
    production: bool,
    local_auth_requested: bool,
    clerk_secret_key: Option<&str>,
    clerk_issuer: Option<&str>,
    clerk_authorized_party: Option<&str>,
) -> Result<HeyVeraAuthMode, String> {
    if production && local_auth_requested {
        return Err("CORTEX_AUTH_DISABLED is forbidden in HeyVera production".into());
    }

    if production {
        if clerk_secret_key.is_none() {
            return Err("CLERK_SECRET_KEY is required in HeyVera production".into());
        }

        let issuer = clerk_issuer.ok_or("CLERK_ISSUER is required in HeyVera production")?;
        if !valid_https_endpoint(issuer) {
            return Err("CLERK_ISSUER must be a non-empty https URL".into());
        }

        let authorized_party = clerk_authorized_party
            .ok_or("CLERK_AUTHORIZED_PARTY is required in HeyVera production")?;
        if !valid_https_origin(authorized_party) {
            return Err("CLERK_AUTHORIZED_PARTY must be a non-empty https origin".into());
        }
    }

    if clerk_secret_key.is_some() {
        Ok(HeyVeraAuthMode::Clerk)
    } else {
        Ok(HeyVeraAuthMode::LocalDevelopment)
    }
}

/// Load and validate the complete HeyVera authentication posture before startup.
pub fn load_heyvera_auth_config() -> Result<HeyVeraAuthConfig, String> {
    let clerk_secret_key = non_empty_env("CLERK_SECRET_KEY");
    let clerk_issuer = non_empty_env("CLERK_ISSUER");
    let clerk_authorized_party = non_empty_env("CLERK_AUTHORIZED_PARTY");
    let mode = validate_auth_config_values(
        is_production_runtime(),
        env_flag("CORTEX_AUTH_DISABLED"),
        clerk_secret_key.as_deref(),
        clerk_issuer.as_deref(),
        clerk_authorized_party.as_deref(),
    )?;

    Ok(HeyVeraAuthConfig {
        mode,
        clerk_secret_key,
    })
}

fn blocked_account_message(status: Option<&str>) -> Option<&'static str> {
    match status {
        Some("suspended") => Some("account suspended"),
        Some("deleted") => Some("account deleted"),
        _ => None,
    }
}

pub(crate) fn account_access_error(app_state: &AppState, user_id: &str) -> Option<&'static str> {
    if user_id == "local" {
        return None;
    }
    let db = app_state.db.as_ref()?;
    let status = db.get_account_status(user_id);
    blocked_account_message(status.as_deref())
}

/// Block suspended/deleted accounts after JWT verification.
fn reject_if_account_blocked(
    app_state: &AppState,
    user_id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    match account_access_error(app_state, user_id) {
        Some(message) => Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: message.into(),
            }),
        )),
        None => Ok(()),
    }
}

/// Configurable TTL for the JWKS cache (default 5 minutes).
fn jwks_ttl_secs() -> u64 {
    std::env::var("CORTEX_JWKS_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300)
}

/// Expected issuer prefix for Clerk JWTs.
fn clerk_issuer_prefix() -> Option<String> {
    non_empty_env("CLERK_ISSUER")
}

/// Authorized party (azp) claim — the Clerk frontend API key or app ID.
fn clerk_authorized_party() -> Option<String> {
    non_empty_env("CLERK_AUTHORIZED_PARTY")
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ClerkClaims {
    sub: String,
    exp: usize,
    /// Issuer — should match the Clerk instance URL.
    #[serde(default)]
    iss: Option<String>,
    /// Authorized party — frontend app that generated the token.
    #[serde(default)]
    azp: Option<String>,
    #[serde(flatten)]
    _extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKey>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwkKey {
    kid: String,
    kty: String,
    n: String,
    e: String,
}

#[derive(Debug, Clone)]
pub struct JwksCache {
    keys: Vec<JwkKey>,
    fetched_at: std::time::Instant,
}

impl JwksCache {
    pub fn empty() -> Self {
        Self {
            keys: Vec::new(),
            fetched_at: std::time::Instant::now() - std::time::Duration::from_secs(7200),
        }
    }

    fn is_stale(&self) -> bool {
        self.fetched_at.elapsed() > std::time::Duration::from_secs(jwks_ttl_secs())
    }
}

/// Stampede guard: prevents multiple concurrent JWKS fetches.
/// When a cache miss occurs, the first caller fetches while others wait on the Notify.
pub struct JwksStampedeGuard {
    pub fetching: AtomicBool,
    pub notify: Notify,
}

impl Default for JwksStampedeGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl JwksStampedeGuard {
    pub fn new() -> Self {
        Self {
            fetching: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }
}

pub async fn fetch_jwks(clerk_secret_key: &str) -> Result<Vec<JwkKey>, String> {
    let start = std::time::Instant::now();
    let client = reqwest::Client::new();
    let res = client
        .get("https://api.clerk.com/v1/jwks")
        .bearer_auth(clerk_secret_key)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(
                method = "jwks_fetch",
                duration_ms = start.elapsed().as_millis() as u64,
                error = %e,
                "JWKS fetch failed"
            );
            format!("JWKS fetch failed: {e}")
        })?;

    if !res.status().is_success() {
        let status = res.status();
        tracing::warn!(
            method = "jwks_fetch",
            duration_ms = start.elapsed().as_millis() as u64,
            http_status = status.as_u16(),
            "JWKS fetch returned non-success status"
        );
        return Err(format!("JWKS fetch returned {}", status));
    }

    let jwks: JwksResponse = res
        .json()
        .await
        .map_err(|e| format!("JWKS parse failed: {e}"))?;
    tracing::info!(
        method = "jwks_fetch",
        duration_ms = start.elapsed().as_millis() as u64,
        key_count = jwks.keys.len(),
        "JWKS refresh"
    );
    Ok(jwks.keys)
}

async fn get_or_refresh_jwks(
    cache: &RwLock<JwksCache>,
    stampede: &JwksStampedeGuard,
    clerk_secret_key: &str,
    force: bool,
) -> Result<Vec<JwkKey>, String> {
    // Fast path: return cached keys if fresh
    {
        let c = cache.read().await;
        if !force && !c.is_stale() && !c.keys.is_empty() {
            return Ok(c.keys.clone());
        }
    }

    // Stampede protection: only one caller fetches, others wait
    if stampede
        .fetching
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        // We won the race — fetch JWKS
        let result = fetch_jwks(clerk_secret_key).await;
        match result {
            Ok(keys) => {
                let mut c = cache.write().await;
                c.keys = keys.clone();
                c.fetched_at = std::time::Instant::now();
                drop(c);
                stampede.fetching.store(false, Ordering::SeqCst);
                stampede.notify.notify_waiters();
                Ok(keys)
            }
            Err(e) => {
                stampede.fetching.store(false, Ordering::SeqCst);
                stampede.notify.notify_waiters();
                Err(e)
            }
        }
    } else {
        // Another caller is fetching — wait for notification then read cache
        stampede.notify.notified().await;
        let c = cache.read().await;
        if c.keys.is_empty() {
            Err("JWKS fetch by another request failed".to_string())
        } else {
            Ok(c.keys.clone())
        }
    }
}

pub async fn get_or_refresh_jwks_pub(
    cache: &RwLock<JwksCache>,
    stampede: &JwksStampedeGuard,
    clerk_secret_key: &str,
    force: bool,
) -> Result<Vec<JwkKey>, String> {
    get_or_refresh_jwks(cache, stampede, clerk_secret_key, force).await
}

pub fn verify_token_pub(token: &str, keys: &[JwkKey]) -> Result<String, String> {
    verify_token(token, keys).map(|claims| claims.sub)
}

fn verify_token(token: &str, keys: &[JwkKey]) -> Result<ClerkClaims, String> {
    let header = decode_header(token).map_err(|e| format!("invalid JWT header: {e}"))?;
    if let Some(token_type) = header.typ.as_deref() {
        if !token_type.eq_ignore_ascii_case("JWT") {
            return Err("unsupported JWT type".into());
        }
    }
    let kid = header.kid.ok_or("JWT missing kid")?;

    let key = keys
        .iter()
        .find(|k| k.kid == kid)
        .ok_or_else(|| format!("no JWK matching kid={kid}"))?;

    if key.kty != "RSA" {
        return Err(format!("unsupported key type: {}", key.kty));
    }

    let decoding_key = DecodingKey::from_rsa_components(&key.n, &key.e)
        .map_err(|e| format!("invalid RSA components: {e}"))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_required_spec_claims(&["exp", "sub"]);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.validate_aud = false;

    let expected_issuer = clerk_issuer_prefix();
    if is_production_runtime() && expected_issuer.is_none() {
        return Err("CLERK_ISSUER is not configured".into());
    }
    if let Some(expected_iss) = expected_issuer {
        validation.set_issuer(&[&expected_iss]);
    }

    let token_data = decode::<ClerkClaims>(token, &decoding_key, &validation)
        .map_err(|e| format!("JWT verification failed: {e}"))?;

    if token_data.claims.sub.trim().is_empty() {
        return Err("JWT subject is empty".into());
    }

    let expected_authorized_party = clerk_authorized_party();
    if is_production_runtime() && expected_authorized_party.is_none() {
        return Err("CLERK_AUTHORIZED_PARTY is not configured".into());
    }
    if let Some(expected_azp) = expected_authorized_party {
        match &token_data.claims.azp {
            Some(azp) if azp == &expected_azp => {}
            Some(azp) => {
                return Err(format!(
                    "JWT azp mismatch: expected {expected_azp}, got {azp}"
                ));
            }
            None => {
                return Err("JWT missing azp claim".to_string());
            }
        }
    }

    Ok(token_data.claims)
}

/// Verify a Clerk JWT and return the user_id. Called by the Soma auth layer
/// for backward-compatible Clerk JWT authentication.
pub async fn verify_clerk_jwt(token: &str, state: &Arc<AppState>) -> Result<String, String> {
    let clerk_secret = state
        .clerk_secret_key
        .as_ref()
        .ok_or("clerk auth not configured")?;

    let keys =
        get_or_refresh_jwks(&state.jwks_cache, &state.jwks_stampede, clerk_secret, false).await?;
    let user_id = match verify_token(token, &keys) {
        Ok(claims) => claims.sub,
        Err(_) => {
            let keys =
                get_or_refresh_jwks(&state.jwks_cache, &state.jwks_stampede, clerk_secret, true)
                    .await?;
            verify_token(token, &keys)?.sub
        }
    };

    if let Some(message) = account_access_error(state, &user_id) {
        return Err(message.into());
    }

    Ok(user_id)
}

impl<S> FromRequestParts<S> for ClerkUser
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<ErrorResponse>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state: Arc<AppState> = Arc::from_ref(state);

        // Try Soma auth first — if the header starts with "Soma ", delegate to extract_identity
        let raw_auth = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if let Some(ref header) = raw_auth {
            if header.starts_with("Soma ") {
                #[cfg(feature = "soma")]
                match crate::soma::extract_identity(parts, &app_state).await {
                    Ok(identity) => {
                        reject_if_account_blocked(&app_state, &identity.user_id)?;
                        return Ok(ClerkUser {
                            user_id: identity.user_id,
                        });
                    }
                    Err(e) => {
                        let msg = format!("{e:?}");
                        return Err((StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: msg })));
                    }
                }

                // With the feature off the `Soma ` scheme is not a credential
                // format this build understands. Reject it rather than falling
                // through to the Clerk verifier, which would report the far
                // more confusing "invalid JWT" for a token that is not a JWT.
                #[cfg(not(feature = "soma"))]
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse {
                        error: "unsupported authorization scheme: Soma".into(),
                    }),
                ));
            }
        }

        let clerk_secret = match &app_state.clerk_secret_key {
            Some(key) => key.clone(),
            None => {
                // No Clerk secret: local/dev only. Production must fail closed.
                if is_production_runtime() {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "clerk auth not configured".into(),
                        }),
                    ));
                }
                return Ok(ClerkUser {
                    user_id: "local".to_string(),
                });
            }
        };

        let token = match raw_auth.as_deref().and_then(|v| v.strip_prefix("Bearer ")) {
            Some(t) => t.to_string(),
            None => {
                if local_auth_allowed() {
                    return Ok(ClerkUser {
                        user_id: "local".to_string(),
                    });
                }
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse {
                        error: "bearer token required".into(),
                    }),
                ));
            }
        };

        // Try with cached JWKS first (with stampede protection)
        let keys = get_or_refresh_jwks(
            &app_state.jwks_cache,
            &app_state.jwks_stampede,
            &clerk_secret,
            false,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e }),
            )
        })?;

        match verify_token(&token, &keys) {
            Ok(claims) => {
                reject_if_account_blocked(&app_state, &claims.sub)?;
                Ok(ClerkUser {
                    user_id: claims.sub,
                })
            }
            Err(_first_err) => {
                // Key rotation: retry with fresh JWKS
                let keys = match get_or_refresh_jwks(
                    &app_state.jwks_cache,
                    &app_state.jwks_stampede,
                    &clerk_secret,
                    true,
                )
                .await
                {
                    Ok(keys) => keys,
                    Err(_) => {
                        return Err((
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(ErrorResponse {
                                error: "clerk jwks unavailable".into(),
                            }),
                        ));
                    }
                };

                let claims = verify_token(&token, &keys).map_err(|_| {
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(ErrorResponse {
                            error: "invalid bearer token".into(),
                        }),
                    )
                })?;
                reject_if_account_blocked(&app_state, &claims.sub)?;
                Ok(ClerkUser {
                    user_id: claims.sub,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: Option<&str> = Some("sk_test_server_secret");
    const ISSUER: Option<&str> = Some("https://clerk.heyvera.org");
    const AUTHORIZED_PARTY: Option<&str> = Some("https://heyvera.org");

    #[test]
    fn production_auth_requires_every_trust_anchor() {
        assert_eq!(
            validate_auth_config_values(true, false, SECRET, ISSUER, AUTHORIZED_PARTY),
            Ok(HeyVeraAuthMode::Clerk)
        );

        let missing_secret =
            validate_auth_config_values(true, false, None, ISSUER, AUTHORIZED_PARTY)
                .expect_err("production must require the Clerk secret");
        assert!(missing_secret.contains("CLERK_SECRET_KEY"));

        let missing_issuer =
            validate_auth_config_values(true, false, SECRET, None, AUTHORIZED_PARTY)
                .expect_err("production must require the issuer");
        assert!(missing_issuer.contains("CLERK_ISSUER"));

        let missing_party = validate_auth_config_values(true, false, SECRET, ISSUER, None)
            .expect_err("production must require the authorized party");
        assert!(missing_party.contains("CLERK_AUTHORIZED_PARTY"));
    }

    #[test]
    fn production_auth_rejects_local_override() {
        let error = validate_auth_config_values(true, true, SECRET, ISSUER, AUTHORIZED_PARTY)
            .expect_err("local auth must never override production");
        assert!(error.contains("CORTEX_AUTH_DISABLED"));
    }

    #[test]
    fn production_auth_requires_https_trust_anchors() {
        let issuer_error = validate_auth_config_values(
            true,
            false,
            SECRET,
            Some("http://clerk.invalid"),
            AUTHORIZED_PARTY,
        )
        .expect_err("issuer must use https");
        assert!(issuer_error.contains("https URL"));

        let party_error =
            validate_auth_config_values(true, false, SECRET, ISSUER, Some("http://heyvera.org"))
                .expect_err("authorized party must use https");
        assert!(party_error.contains("https origin"));

        for invalid in [
            "https://",
            "https://user@heyvera.org",
            "https://heyvera.org?redirect=evil",
            "https://heyvera.org/#fragment",
        ] {
            assert!(
                !valid_https_endpoint(invalid),
                "{invalid} must not be accepted as a trust anchor"
            );
        }
        assert!(!valid_https_origin("https://heyvera.org/app"));
    }

    #[test]
    fn development_keeps_explicit_local_mode_available() {
        assert_eq!(
            validate_auth_config_values(false, true, None, None, None),
            Ok(HeyVeraAuthMode::LocalDevelopment)
        );
        assert_eq!(
            validate_auth_config_values(false, false, SECRET, None, None),
            Ok(HeyVeraAuthMode::Clerk)
        );
    }

    #[test]
    fn blocked_account_status_is_centralized() {
        assert_eq!(
            blocked_account_message(Some("suspended")),
            Some("account suspended")
        );
        assert_eq!(
            blocked_account_message(Some("deleted")),
            Some("account deleted")
        );
        assert_eq!(blocked_account_message(Some("active")), None);
        assert_eq!(blocked_account_message(None), None);
    }
}

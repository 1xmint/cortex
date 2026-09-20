use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::lock::LockRecovering;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Sliding window counter
// ---------------------------------------------------------------------------

/// A sliding-window rate limiter entry.
/// Tracks request counts in two adjacent windows to approximate a smooth rate.
struct SlidingWindow {
    /// Start of the current window.
    window_start: Instant,
    /// Duration of each window.
    window_secs: u64,
    /// Requests in the previous (completed) window.
    prev_count: u32,
    /// Requests in the current window.
    curr_count: u32,
    /// Maximum requests allowed per window.
    max_requests: u32,
}

impl SlidingWindow {
    fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            window_start: Instant::now(),
            window_secs,
            prev_count: 0,
            curr_count: 0,
            max_requests,
        }
    }

    /// Advance windows if time has passed.
    fn advance(&mut self) {
        let elapsed = self.window_start.elapsed().as_secs();
        if elapsed >= self.window_secs * 2 {
            // Both windows expired
            self.prev_count = 0;
            self.curr_count = 0;
            self.window_start = Instant::now();
        } else if elapsed >= self.window_secs {
            // Current window becomes previous
            self.prev_count = self.curr_count;
            self.curr_count = 0;
            self.window_start = Instant::now();
        }
    }

    /// Estimated request count in the sliding window.
    fn estimated_count(&self) -> f64 {
        let elapsed = self.window_start.elapsed().as_secs_f64();
        let weight = 1.0 - (elapsed / self.window_secs as f64).min(1.0);
        (self.prev_count as f64 * weight) + self.curr_count as f64
    }

    /// Try to record a request. Returns Ok(()) or Err(retry_after_secs).
    fn try_acquire(&mut self) -> Result<(), f64> {
        self.advance();
        if self.estimated_count() >= self.max_requests as f64 {
            // Seconds until the previous window fully expires
            let elapsed = self.window_start.elapsed().as_secs_f64();
            let retry_after = (self.window_secs as f64 - elapsed).max(1.0);
            Err(retry_after)
        } else {
            self.curr_count += 1;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Endpoint-specific rate limit categories
// ---------------------------------------------------------------------------

/// Rate limit category for different endpoint types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitCategory {
    /// General read endpoints (default)
    Read,
    /// General write endpoints (POST/PUT/PATCH/DELETE)
    Write,
    /// Post creation — 10/hour
    PostCreate,
    /// Profile edits — 5/hour
    ProfileEdit,
    /// Follow/unfollow — 30/hour
    FollowUnfollow,
    /// Reactions (likes, bookmarks, reposts) — 60/hour
    Reaction,
    /// API key reads (list keys) — 30/min
    KeyRead,
    /// API key writes (save/delete keys) — 10/hour
    KeyWrite,
}

impl RateLimitCategory {
    /// (max_requests, window_seconds)
    fn limits(self) -> (u32, u64) {
        match self {
            Self::Read => (120, 60),            // 120/min
            Self::Write => (60, 60),            // 60/min
            Self::PostCreate => (10, 3600),     // 10/hour
            Self::ProfileEdit => (5, 3600),     // 5/hour
            Self::FollowUnfollow => (30, 3600), // 30/hour
            Self::Reaction => (60, 3600),       // 60/hour
            Self::KeyRead => (30, 60),          // 30/min
            Self::KeyWrite => (10, 3600),       // 10/hour
        }
    }
}

/// Classify a request into a rate limit category based on path and method.
fn classify_request(method: &Method, path: &str) -> RateLimitCategory {
    // API key management endpoints
    if path.starts_with("/api/keys") {
        return match *method {
            Method::GET | Method::HEAD | Method::OPTIONS => RateLimitCategory::KeyRead,
            _ => RateLimitCategory::KeyWrite,
        };
    }

    // Social-specific endpoints
    if path.starts_with("/v1/social/posts")
        && method == Method::POST
        && !path.contains("/like")
        && !path.contains("/bookmark")
        && !path.contains("/repost")
    {
        return RateLimitCategory::PostCreate;
    }
    // Pulse draft create counts as post-like write pressure
    if path == "/v1/pulse/drafts" && method == Method::POST {
        return RateLimitCategory::PostCreate;
    }
    if (path.starts_with("/v1/social/profile")
        || path.starts_with("/v1/social/profiles")
        || path.starts_with("/v1/social/me/profile"))
        && matches!(*method, Method::POST | Method::PUT | Method::PATCH)
    {
        return RateLimitCategory::ProfileEdit;
    }
    if path.starts_with("/v1/social/follows")
        || path.contains("/follow")
        || path.contains("/unfollow")
    {
        return RateLimitCategory::FollowUnfollow;
    }
    if path.contains("/like")
        || path.contains("/bookmark")
        || path.contains("/repost")
        || path.contains("/react")
    {
        return RateLimitCategory::Reaction;
    }

    // General classification by method
    match *method {
        Method::GET | Method::HEAD | Method::OPTIONS => RateLimitCategory::Read,
        _ => RateLimitCategory::Write,
    }
}

// ---------------------------------------------------------------------------
// Per-key rate limiter
// ---------------------------------------------------------------------------

struct BucketEntry {
    windows: HashMap<RateLimitCategory, SlidingWindow>,
    last_access: Instant,
}

pub struct RateLimiter {
    /// Per-account/user rate limit buckets (keyed by user identity).
    account_buckets: Mutex<HashMap<String, BucketEntry>>,
    /// Per-IP rate limit buckets (layered on top of account limits).
    ip_buckets: Mutex<HashMap<String, BucketEntry>>,
}

/// Global IP rate limits: 200 requests/min for reads, 100/min for writes.
const IP_READ_LIMIT: u32 = 200;
const IP_WRITE_LIMIT: u32 = 100;
const IP_WINDOW_SECS: u64 = 60;

impl RateLimiter {
    pub fn new(_max_requests: u32, _per_seconds: u64) -> Self {
        Self {
            account_buckets: Mutex::new(HashMap::new()),
            ip_buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Default rate limiter (compatibility constructor).
    pub fn default_per_user() -> Self {
        Self::new(60, 60)
    }

    /// Check account-level rate limit for a specific category.
    pub fn check_account(&self, key: &str, category: RateLimitCategory) -> Result<(), f64> {
        let (max_requests, window_secs) = category.limits();
        let mut buckets = self.account_buckets.lock_recovering();
        let entry = buckets
            .entry(key.to_string())
            .or_insert_with(|| BucketEntry {
                windows: HashMap::new(),
                last_access: Instant::now(),
            });
        entry.last_access = Instant::now();
        let window = entry
            .windows
            .entry(category)
            .or_insert_with(|| SlidingWindow::new(max_requests, window_secs));
        window.try_acquire()
    }

    /// Check IP-level rate limit (coarser, prevents abuse from single IPs).
    pub fn check_ip(&self, ip: &str, is_write: bool) -> Result<(), f64> {
        let limit = if is_write {
            IP_WRITE_LIMIT
        } else {
            IP_READ_LIMIT
        };
        let category = if is_write {
            RateLimitCategory::Write
        } else {
            RateLimitCategory::Read
        };

        let mut buckets = self.ip_buckets.lock_recovering();
        let entry = buckets
            .entry(ip.to_string())
            .or_insert_with(|| BucketEntry {
                windows: HashMap::new(),
                last_access: Instant::now(),
            });
        entry.last_access = Instant::now();
        let window = entry
            .windows
            .entry(category)
            .or_insert_with(|| SlidingWindow::new(limit, IP_WINDOW_SECS));
        window.try_acquire()
    }

    /// Backward-compatible check (uses Write category).
    pub fn check(&self, key: &str) -> Result<(), f64> {
        self.check_account(key, RateLimitCategory::Write)
    }

    /// Remove stale entries (not accessed in 10+ minutes).
    pub fn cleanup(&self) {
        let cutoff = Instant::now() - std::time::Duration::from_secs(600);

        {
            let mut buckets = self.account_buckets.lock_recovering();
            buckets.retain(|_, entry| entry.last_access > cutoff);
        }
        {
            let mut buckets = self.ip_buckets.lock_recovering();
            buckets.retain(|_, entry| entry.last_access > cutoff);
        }
    }
}

// ---------------------------------------------------------------------------
// Request identity extraction
// ---------------------------------------------------------------------------

/// Extract user identity from the Authorization header for rate limiting.
/// Handles both Bearer JWT and Soma delegation tokens.
/// Falls back to IP-based key, then "anonymous".
fn extract_user_key(req: &Request<axum::body::Body>) -> String {
    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    if let Some(header) = auth {
        // Agent API key: bucket by key prefix (stable per-agent rate limit)
        if let Some(token) = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("Agent "))
        {
            if token.starts_with("hvak_") {
                let take = token.len().min(20);
                return format!("agent:{}", &token[..take]);
            }
        }

        // Bearer JWT: extract `sub` claim without full verification
        if let Some(token) = header.strip_prefix("Bearer ") {
            let parts: Vec<&str> = token.splitn(3, '.').collect();
            if parts.len() >= 2 {
                if let Some(payload) = base64url_decode(parts[1]) {
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) {
                        if let Some(sub) = value.get("sub").and_then(|s| s.as_str()) {
                            return sub.to_string();
                        }
                    }
                }
            }
        }

        // Soma delegation: extract subject_did from the JSON token
        if let Some(token) = header.strip_prefix("Soma ") {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(token) {
                if let Some(did) = value.get("subject_did").and_then(|s| s.as_str()) {
                    return did.to_string();
                }
            }
        }
    }

    // API key: use the key itself as the rate limit bucket
    if let Some(api_key) = req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) {
        return format!("apikey:{}", &api_key[..api_key.len().min(16)]);
    }

    "anonymous".to_string()
}

/// Extract the client IP from request headers.
/// Checks X-Forwarded-For, X-Real-IP, then falls back to "unknown".
fn extract_client_ip(req: &Request<axum::body::Body>) -> String {
    // X-Forwarded-For: first IP in chain is the client
    if let Some(xff) = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(first_ip) = xff.split(',').next().map(|s| s.trim()) {
            if !first_ip.is_empty() {
                return first_ip.to_string();
            }
        }
    }

    // X-Real-IP
    if let Some(real_ip) = req.headers().get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let trimmed = real_ip.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    "unknown".to_string()
}

/// Minimal base64url decode (no padding required).
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    // Replace URL-safe chars with standard base64 chars
    let mut s = input.replace('-', "+").replace('_', "/");
    // Add padding
    match s.len() % 4 {
        2 => s.push_str("=="),
        3 => s.push('='),
        0 => {}
        _ => return None,
    }
    // Use a simple decode — we only need this for JWT payload extraction
    general_base64_decode(&s)
}

fn general_base64_decode(input: &str) -> Option<Vec<u8>> {
    static TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }

    let mut output = Vec::with_capacity(bytes.len() * 3 / 4);

    for chunk in bytes.chunks(4) {
        let mut buf = [0u8; 4];
        let mut pad = 0;
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                buf[i] = 0;
                pad += 1;
            } else {
                let pos = TABLE.iter().position(|&c| c == b)?;
                buf[i] = pos as u8;
            }
        }

        output.push((buf[0] << 2) | (buf[1] >> 4));
        if pad < 2 {
            output.push((buf[1] << 4) | (buf[2] >> 2));
        }
        if pad < 1 {
            output.push((buf[2] << 6) | buf[3]);
        }
    }

    Some(output)
}

// ---------------------------------------------------------------------------
// Axum middleware
// ---------------------------------------------------------------------------

/// Axum middleware: per-IP + per-account rate limiting with endpoint-specific limits.
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let client_ip = extract_client_ip(&req);
    let user_key = extract_user_key(&req);
    let category = classify_request(&method, &path);

    let is_write = !matches!(category, RateLimitCategory::Read);

    // Layer 1: Per-IP rate limit
    if let Err(retry_after) = state.rate_limiter.check_ip(&client_ip, is_write) {
        return rate_limit_response(retry_after);
    }

    // Layer 2: Per-account/user rate limit (endpoint-specific)
    if let Err(retry_after) = state.rate_limiter.check_account(&user_key, category) {
        return rate_limit_response(retry_after);
    }

    next.run(req).await
}

/// Build a 429 response with Retry-After header.
fn rate_limit_response(retry_after: f64) -> Response {
    let retry_secs = retry_after.ceil() as u64;
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        serde_json::json!({
            "error": "rate limit exceeded",
            "retry_after": retry_secs
        })
        .to_string(),
    )
        .into_response();
    response.headers_mut().insert(
        "retry-after",
        HeaderValue::from_str(&retry_secs.to_string()).unwrap(),
    );
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allows_within_limit() {
        let limiter = RateLimiter::new(5, 60);
        for _ in 0..5 {
            assert!(limiter.check("user1").is_ok());
        }
    }

    #[test]
    fn test_separate_users() {
        let limiter = RateLimiter::new(2, 60);
        assert!(limiter.check("user1").is_ok());
        assert!(limiter.check("user1").is_ok());
        // user2 should still have tokens
        assert!(limiter.check("user2").is_ok());
    }

    #[test]
    fn test_cleanup_preserves_recent() {
        let limiter = RateLimiter::new(10, 60);
        limiter.check("user1").ok();
        limiter.cleanup();
        // recent bucket should survive cleanup
        assert!(limiter.check("user1").is_ok());
    }

    #[test]
    fn test_base64url_decode() {
        // "hello" in base64url
        let encoded = "aGVsbG8";
        let decoded = base64url_decode(encoded).unwrap();
        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn test_classify_post_create() {
        assert_eq!(
            classify_request(&Method::POST, "/v1/social/posts"),
            RateLimitCategory::PostCreate
        );
    }

    #[test]
    fn test_classify_read() {
        assert_eq!(
            classify_request(&Method::GET, "/api/health"),
            RateLimitCategory::Read
        );
    }

    #[test]
    fn test_classify_follow() {
        assert_eq!(
            classify_request(&Method::POST, "/v1/social/follow"),
            RateLimitCategory::FollowUnfollow
        );
    }

    #[test]
    fn test_ip_rate_limit() {
        let limiter = RateLimiter::new(60, 60);
        // Should allow up to IP_READ_LIMIT reads
        for _ in 0..IP_READ_LIMIT {
            assert!(limiter.check_ip("1.2.3.4", false).is_ok());
        }
        // Next one should fail
        assert!(limiter.check_ip("1.2.3.4", false).is_err());
        // Different IP should still work
        assert!(limiter.check_ip("5.6.7.8", false).is_ok());
    }

    #[test]
    fn test_endpoint_specific_limits() {
        let limiter = RateLimiter::new(60, 60);
        // Post creation: 10/hour
        for _ in 0..10 {
            assert!(limiter
                .check_account("user1", RateLimitCategory::PostCreate)
                .is_ok());
        }
        assert!(limiter
            .check_account("user1", RateLimitCategory::PostCreate)
            .is_err());
        // But reads should still work for the same user
        assert!(limiter
            .check_account("user1", RateLimitCategory::Read)
            .is_ok());
    }
}

//! The Cortex worker service credential (`cwk_` keys).
//!
//! # What this is for
//!
//! A worker is a headless daemon on a box somewhere. It has no browser, no
//! interactive login, and no way to obtain or renew a Clerk *user* JWT — so
//! before this module the only worker credential `authenticate_worker` would
//! accept in practice was the empty string, and it accepted that only because
//! `CLERK_SECRET_KEY` was unset. Production therefore had both halves of the
//! problem at once: no legitimate worker could authenticate, and any client at
//! all could.
//!
//! A `cwk_` key is a service credential. It belongs to a user, so the work it
//! dispatches is still attributed and billed; it carries a coarse scope; and it
//! can be expired or revoked on its own without touching that user's account.
//!
//! # What this is deliberately not
//!
//! It is not `agent_auth`. That module's `hvak_` keys resolve against
//! `social_linked_agents` and yield an `AgentIdentity` — a HeyVera Socials
//! principal with a profile and a slug. Cortex workers have nothing to do with
//! social profiles, and reusing that identity would tie the two products'
//! auth together at exactly the layer where they must stay apart. What the two
//! genuinely share is the key material itself, which lives in
//! [`crate::key_material`] and is product-neutral.
//!
//! # Issuance
//!
//! There is no HTTP route here on purpose. Keys are minted by the
//! `cortex-worker-key` binary, which prints the plaintext once and never writes
//! it anywhere. A public self-serve issuance endpoint is a separate change with
//! its own authorization story.

use crate::key_material::{self, IssuedKey};

/// Scheme tag for Cortex worker keys.
pub const WORKER_KEY_PREFIX: &str = "cwk_";

/// Environment variable that re-opens the anonymous worker path.
///
/// Unset — the default, and the only sane production value — an unauthenticated
/// worker is refused. Set to `1`, an empty token authenticates as the `local`
/// user, which is what local development against a no-Clerk API needs.
///
/// It is an explicit opt-in rather than "Clerk happens to be unconfigured"
/// because those are not the same statement. A deployment can lose its
/// `CLERK_SECRET_KEY` by accident; nobody sets this variable by accident.
pub const ALLOW_ANONYMOUS_WORKER_ENV: &str = "CORTEX_ALLOW_ANONYMOUS_WORKER";

/// Principal granted to an anonymous worker when the escape hatch is open.
pub const ANONYMOUS_WORKER_USER: &str = "local";

/// Default scope for a newly issued worker key.
pub const DEFAULT_WORKER_SCOPE: &str = "worker";

/// Mint a new worker key. The plaintext in the returned [`IssuedKey`] is the
/// only copy that will ever exist.
pub fn generate_worker_key() -> IssuedKey {
    key_material::generate_api_key(WORKER_KEY_PREFIX)
}

/// Does this token claim to be a worker key?
///
/// A claim, not a proof. Used only to route a token to the worker-key branch
/// rather than the JWT verifier; the actual decision is the database lookup.
pub fn is_worker_key(token: &str) -> bool {
    token.starts_with(WORKER_KEY_PREFIX)
}

/// SHA-256 hex of a presented worker key, for lookup against `worker_keys`.
pub fn hash_worker_key(token: &str) -> String {
    key_material::hash_api_key(token)
}

/// Read the anonymous-worker escape hatch from the environment.
///
/// Called once at [`crate::state::AppState`] construction and stored on the
/// state, so the decision cannot vary between two frames of one connection and
/// so tests can set it without touching process-global environment.
pub fn anonymous_worker_allowed_from_env() -> bool {
    std::env::var(ALLOW_ANONYMOUS_WORKER_ENV)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_key_is_recognisable_and_hashes_to_itself() {
        let key = generate_worker_key();
        assert!(is_worker_key(&key.secret));
        assert_eq!(hash_worker_key(&key.secret), key.hash);
    }

    /// The routing predicate must not claim a Socials key, or a JWT, or the
    /// empty token. Each of those has its own branch and its own verifier.
    #[test]
    fn only_cwk_tokens_are_claimed_as_worker_keys() {
        assert!(!is_worker_key(""));
        assert!(!is_worker_key("hvak_abcdefabcdef"));
        assert!(!is_worker_key("eyJhbGciOiJSUzI1NiJ9.e30.x"));
        assert!(!is_worker_key("{\"issuer_did\":\"did:soma:x\"}"));
        // Close but not the scheme: no underscore, so not a worker key.
        assert!(!is_worker_key("cwkabcdef"));
    }
}

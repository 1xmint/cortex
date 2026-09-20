//! The narrow boundary between an untrusted model client and one supplier.
//!
//! The gateway owns the supplier credential. Its caller owns only a signed,
//! short-lived capability and cannot choose a different tenant, run, attempt,
//! model, price list, or spending authorization. Every transport call follows
//! a durable reservation; an ambiguous outcome stays reserved.

use std::future::Future;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use thiserror::Error;

use crate::db::{Database, ProviderReservation};

const CAPABILITY_VERSION: u32 = 1;
const SUPPORTED_PROVIDER: &str = "claude";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayCapability {
    pub version: u32,
    pub authorization_id: String,
    pub tenant_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub provider: String,
    pub model: String,
    pub expires_at_ms: i64,
}

impl GatewayCapability {
    pub fn new(
        authorization_id: impl Into<String>,
        tenant_id: impl Into<String>,
        run_id: impl Into<String>,
        attempt_id: impl Into<String>,
        model: impl Into<String>,
        expires_at_ms: i64,
    ) -> Self {
        Self {
            version: CAPABILITY_VERSION,
            authorization_id: authorization_id.into(),
            tenant_id: tenant_id.into(),
            run_id: run_id.into(),
            attempt_id: attempt_id.into(),
            provider: SUPPORTED_PROVIDER.to_string(),
            model: model.into(),
            expires_at_ms,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SignedCapability(String);

impl SignedCapability {
    pub fn from_exposed(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SignedCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SignedCapability([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayRequest {
    pub request_key: String,
    pub tenant_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub model: String,
    pub max_output_tokens: i64,
    pub body: Value,
    pub capability: SignedCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransportResponse {
    pub body: Value,
    pub upstream_request_id: Option<String>,
    pub usage: Option<ObservedUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailureKind {
    /// The transport can prove no bytes reached the supplier.
    NotSent,
    /// A timeout says nothing about whether the supplier accepted the call.
    Timeout,
    /// Any other ambiguous network or supplier failure.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFailure {
    pub kind: TransportFailureKind,
    pub upstream_request_id: Option<String>,
    pub message: String,
}

pub trait ProviderTransport {
    fn forward(
        &self,
        supplier_key: &str,
        request: &GatewayRequest,
    ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send;
}

#[derive(Debug, Clone, PartialEq)]
pub struct GatewayOutcome {
    pub body: Option<Value>,
    pub reservation: ProviderReservation,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GatewayError {
    #[error("gateway capability is malformed or has an invalid signature")]
    InvalidCapability,
    #[error("gateway capability has expired")]
    ExpiredCapability,
    #[error("gateway supports only the configured single provider")]
    UnsupportedProvider,
    #[error("request does not match its tenant/run/attempt/model capability")]
    ScopeMismatch,
    #[error("request form is unbounded: {0}")]
    UnboundedRequest(String),
    #[error("no immutable rate exists for this authorization and model")]
    MissingRate,
    #[error("request cost overflowed integer accounting")]
    CostOverflow,
    #[error("spend reservation refused: {0}")]
    Reservation(String),
    #[error("supplier transport failed after reservation: {0}")]
    Transport(String),
    #[error("supplier response contained gateway credential material")]
    CredentialExposure,
    #[error("usage reconciliation failed: {0}")]
    Reconciliation(String),
}

pub struct ProviderGateway<'a, T> {
    db: &'a Database,
    signing_key: &'a [u8],
    supplier_key: &'a str,
    transport: T,
}

impl<'a, T: ProviderTransport> ProviderGateway<'a, T> {
    pub fn new(
        db: &'a Database,
        signing_key: &'a [u8],
        supplier_key: &'a str,
        transport: T,
    ) -> Self {
        Self {
            db,
            signing_key,
            supplier_key,
            transport,
        }
    }

    pub fn issue_capability(
        &self,
        claims: &GatewayCapability,
    ) -> Result<SignedCapability, GatewayError> {
        sign_capability(self.signing_key, claims)
    }

    pub fn verified_claims(
        &self,
        signed: &SignedCapability,
    ) -> Result<GatewayCapability, GatewayError> {
        verify_capability(self.signing_key, signed)
    }

    pub async fn forward(
        &self,
        request: GatewayRequest,
        now_ms: i64,
    ) -> Result<GatewayOutcome, GatewayError> {
        let claims = verify_capability(self.signing_key, &request.capability)?;
        validate_scope(&claims, &request, now_ms)?;
        validate_bounded_form(&request)?;

        let rate = self
            .db
            .gateway_model_rate(
                &claims.authorization_id,
                &claims.provider,
                &claims.model,
                now_ms,
            )
            .ok_or(GatewayError::MissingRate)?;
        let input_upper_bound = i64::try_from(
            serde_json::to_vec(&request.body)
                .map_err(|_| GatewayError::UnboundedRequest("body is not serializable".into()))?
                .len(),
        )
        .map_err(|_| GatewayError::CostOverflow)?;
        if input_upper_bound
            .checked_add(request.max_output_tokens)
            .is_none_or(|tokens| tokens > rate.context_window)
        {
            return Err(GatewayError::UnboundedRequest(
                "conservative input bound plus maximum output exceeds the model context".into(),
            ));
        }
        let reserved_micros = upper_bound_cost(
            input_upper_bound,
            request.max_output_tokens,
            rate.input_micros_per_1k,
            rate.output_micros_per_1k,
        )?;
        let request_digest = request_digest(&request)?;

        let reservation = self
            .db
            .reserve_provider_request(
                &claims,
                &request.request_key,
                &request_digest,
                reserved_micros,
                now_ms,
            )
            .map_err(GatewayError::Reservation)?;

        // A replay of a request whose outcome is already known must never call
        // the supplier again. An unresolved replay also stays unresolved: a
        // timeout is not permission to spend the same authorization twice.
        if reservation.status != "reserved" || reservation.replayed {
            return Ok(GatewayOutcome {
                body: None,
                reservation,
            });
        }

        match self.transport.forward(self.supplier_key, &request).await {
            Ok(response) => {
                if response.body.to_string().contains(self.supplier_key) {
                    let reservation = self
                        .db
                        .mark_provider_request_unresolved(
                            &request.request_key,
                            response.upstream_request_id.as_deref(),
                            "supplier response contained credential material",
                            now_ms,
                        )
                        .map_err(GatewayError::Reconciliation)?;
                    return Err(if reservation.status == "unresolved" {
                        GatewayError::CredentialExposure
                    } else {
                        GatewayError::Reconciliation(
                            "could not preserve credential-exposure reservation".into(),
                        )
                    });
                }

                let Some(usage) = response.usage else {
                    let reservation = self
                        .db
                        .mark_provider_request_unresolved(
                            &request.request_key,
                            response.upstream_request_id.as_deref(),
                            "supplier response omitted usage",
                            now_ms,
                        )
                        .map_err(GatewayError::Reconciliation)?;
                    return Ok(GatewayOutcome {
                        body: Some(response.body),
                        reservation,
                    });
                };
                if usage.input_tokens < 0
                    || usage.cached_input_tokens < 0
                    || usage.output_tokens < 0
                    || usage.cached_input_tokens > usage.input_tokens
                {
                    self.db
                        .mark_provider_request_unresolved(
                            &request.request_key,
                            response.upstream_request_id.as_deref(),
                            "supplier returned invalid usage counters",
                            now_ms,
                        )
                        .map_err(GatewayError::Reconciliation)?;
                    return Err(GatewayError::Reconciliation(
                        "supplier returned invalid usage counters".into(),
                    ));
                }

                let observed_micros = rate.cost_micros(
                    usage.input_tokens,
                    usage.cached_input_tokens,
                    usage.output_tokens,
                );
                let reservation = self
                    .db
                    .settle_provider_request(
                        &request.request_key,
                        observed_micros,
                        response.upstream_request_id.as_deref(),
                        now_ms,
                    )
                    .map_err(GatewayError::Reconciliation)?;
                Ok(GatewayOutcome {
                    body: Some(response.body),
                    reservation,
                })
            }
            Err(failure) => {
                let reservation = match failure.kind {
                    TransportFailureKind::NotSent => self.db.release_provider_request(
                        &request.request_key,
                        &failure.message,
                        now_ms,
                    ),
                    TransportFailureKind::Timeout | TransportFailureKind::Unknown => {
                        self.db.mark_provider_request_unresolved(
                            &request.request_key,
                            failure.upstream_request_id.as_deref(),
                            &failure.message,
                            now_ms,
                        )
                    }
                }
                .map_err(GatewayError::Reconciliation)?;
                Err(GatewayError::Transport(format!(
                    "{} ({})",
                    failure.message, reservation.status
                )))
            }
        }
    }
}

pub fn sign_capability(
    signing_key: &[u8],
    claims: &GatewayCapability,
) -> Result<SignedCapability, GatewayError> {
    let payload = serde_json::to_vec(claims).map_err(|_| GatewayError::InvalidCapability)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(signing_key).map_err(|_| GatewayError::InvalidCapability)?;
    mac.update(&payload);
    let signature = mac.finalize().into_bytes();
    Ok(SignedCapability(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature)
    )))
}

fn verify_capability(
    signing_key: &[u8],
    signed: &SignedCapability,
) -> Result<GatewayCapability, GatewayError> {
    let (payload, signature) = signed
        .0
        .split_once('.')
        .ok_or(GatewayError::InvalidCapability)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| GatewayError::InvalidCapability)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| GatewayError::InvalidCapability)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(signing_key).map_err(|_| GatewayError::InvalidCapability)?;
    mac.update(&payload);
    mac.verify_slice(&signature)
        .map_err(|_| GatewayError::InvalidCapability)?;
    serde_json::from_slice(&payload).map_err(|_| GatewayError::InvalidCapability)
}

fn validate_scope(
    claims: &GatewayCapability,
    request: &GatewayRequest,
    now_ms: i64,
) -> Result<(), GatewayError> {
    if claims.version != CAPABILITY_VERSION || claims.provider != SUPPORTED_PROVIDER {
        return Err(GatewayError::UnsupportedProvider);
    }
    if claims.expires_at_ms <= now_ms {
        return Err(GatewayError::ExpiredCapability);
    }
    if claims.tenant_id != request.tenant_id
        || claims.run_id != request.run_id
        || claims.attempt_id != request.attempt_id
        || claims.model != request.model
    {
        return Err(GatewayError::ScopeMismatch);
    }
    Ok(())
}

fn validate_bounded_form(request: &GatewayRequest) -> Result<(), GatewayError> {
    if request.request_key.trim().is_empty() {
        return Err(GatewayError::UnboundedRequest(
            "request key is required".into(),
        ));
    }
    if request.max_output_tokens <= 0 {
        return Err(GatewayError::UnboundedRequest(
            "max_output_tokens must be positive".into(),
        ));
    }
    if request.body.get("model").and_then(Value::as_str) != Some(request.model.as_str()) {
        return Err(GatewayError::UnboundedRequest(
            "body model must exactly match the capability model".into(),
        ));
    }
    if request.body.get("max_tokens").and_then(Value::as_i64) != Some(request.max_output_tokens) {
        return Err(GatewayError::UnboundedRequest(
            "body max_tokens must exactly match the reserved maximum".into(),
        ));
    }
    validate_tools(request.body.get("tools"))?;
    if request
        .body
        .get("stream")
        .is_some_and(|stream| !stream.is_boolean())
    {
        return Err(GatewayError::UnboundedRequest(
            "stream must be a boolean".into(),
        ));
    }
    Ok(())
}

fn validate_tools(tools: Option<&Value>) -> Result<(), GatewayError> {
    let Some(tools) = tools else {
        return Ok(());
    };
    let Some(tools) = tools.as_array() else {
        return Err(GatewayError::UnboundedRequest(
            "tools must be an array".into(),
        ));
    };
    if tools.len() > 64 {
        return Err(GatewayError::UnboundedRequest(
            "tools exceed the 64-definition bound".into(),
        ));
    }
    let mut names = std::collections::HashSet::new();
    for tool in tools {
        let Some(tool) = tool.as_object() else {
            return Err(GatewayError::UnboundedRequest(
                "each tool definition must be an object".into(),
            ));
        };
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            return Err(GatewayError::UnboundedRequest(
                "each tool definition requires a name".into(),
            ));
        };
        if name.is_empty() || name.len() > 128 || !names.insert(name) {
            return Err(GatewayError::UnboundedRequest(
                "tool names must be unique and between 1 and 128 bytes".into(),
            ));
        }
        if tool
            .get("input_schema")
            .and_then(Value::as_object)
            .is_none()
        {
            return Err(GatewayError::UnboundedRequest(
                "each tool definition requires an object input_schema".into(),
            ));
        }
        if tool
            .get("description")
            .is_some_and(|description| !description.is_string())
        {
            return Err(GatewayError::UnboundedRequest(
                "tool descriptions must be strings".into(),
            ));
        }
    }
    Ok(())
}

fn upper_bound_cost(
    input_tokens: i64,
    output_tokens: i64,
    input_micros_per_1k: i64,
    output_micros_per_1k: i64,
) -> Result<i64, GatewayError> {
    fn ceiling_per_thousand(tokens: i64, rate: i64) -> Option<i64> {
        tokens
            .checked_mul(rate)?
            .checked_add(999)?
            .checked_div(1_000)
    }
    let input = ceiling_per_thousand(input_tokens, input_micros_per_1k)
        .ok_or(GatewayError::CostOverflow)?;
    let output = ceiling_per_thousand(output_tokens, output_micros_per_1k)
        .ok_or(GatewayError::CostOverflow)?;
    input
        .checked_add(output)
        .filter(|cost| *cost > 0)
        .ok_or(GatewayError::CostOverflow)
}

fn request_digest(request: &GatewayRequest) -> Result<String, GatewayError> {
    use sha2::Digest as _;

    let mut digest = Sha256::new();
    for field in [
        request.tenant_id.as_bytes(),
        request.run_id.as_bytes(),
        request.attempt_id.as_bytes(),
        request.model.as_bytes(),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    digest.update(request.max_output_tokens.to_be_bytes());
    digest.update(
        serde_json::to_vec(&request.body)
            .map_err(|_| GatewayError::UnboundedRequest("body is not serializable".into()))?,
    );
    Ok(format!("sha256:{}", hex::encode(digest.finalize())))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::db::SpendAuthorization;

    const NOW: i64 = 1_800_000_000_000;
    const MODEL: &str = "claude-sonnet-5";

    #[derive(Clone)]
    struct StubTransport<'a> {
        db: &'a Database,
        calls: Arc<AtomicUsize>,
        response: Result<TransportResponse, TransportFailure>,
    }

    impl ProviderTransport for StubTransport<'_> {
        fn forward(
            &self,
            supplier_key: &str,
            _request: &GatewayRequest,
        ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
            assert_eq!(supplier_key, "supplier-secret");
            assert_eq!(
                self.db
                    .get_provider_reservation(&_request.request_key)
                    .unwrap()
                    .status,
                "reserved",
                "transport was invoked before its durable reservation"
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(self.response.clone())
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        db: Database,
        claims: GatewayCapability,
        calls: Arc<AtomicUsize>,
    }

    impl Fixture {
        fn new(max_micro_usd: i64, capacity_micro_usd: i64) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = Database::open(&dir.path().join("gateway.sqlite"));
            let price_list_id = db.active_price_list().unwrap().id;
            db.set_supplier_capacity(SUPPORTED_PROVIDER, capacity_micro_usd, NOW)
                .unwrap();
            let authorization = SpendAuthorization {
                id: "auth-1".into(),
                user_id: "tenant-1".into(),
                run_id: "run-1".into(),
                attempt_id: "attempt-1".into(),
                provider: SUPPORTED_PROVIDER.into(),
                model: MODEL.into(),
                price_list_id,
                max_micro_usd,
                expires_at_ms: NOW + 60_000,
            };
            db.create_spend_authorization(&authorization, NOW).unwrap();
            Self {
                _dir: dir,
                db,
                claims: GatewayCapability::new(
                    authorization.id,
                    authorization.user_id,
                    authorization.run_id,
                    authorization.attempt_id,
                    authorization.model,
                    authorization.expires_at_ms,
                ),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn request(
            &self,
            gateway: &ProviderGateway<'_, StubTransport<'_>>,
            request_key: &str,
        ) -> GatewayRequest {
            GatewayRequest {
                request_key: request_key.into(),
                tenant_id: self.claims.tenant_id.clone(),
                run_id: self.claims.run_id.clone(),
                attempt_id: self.claims.attempt_id.clone(),
                model: self.claims.model.clone(),
                max_output_tokens: 100,
                body: serde_json::json!({
                    "model": MODEL,
                    "max_tokens": 100,
                    "messages": [{"role": "user", "content": "bounded stub request"}]
                }),
                capability: gateway.issue_capability(&self.claims).unwrap(),
            }
        }

        fn gateway(
            &self,
            response: Result<TransportResponse, TransportFailure>,
        ) -> ProviderGateway<'_, StubTransport<'_>> {
            ProviderGateway::new(
                &self.db,
                b"test-signing-key-with-enough-entropy",
                "supplier-secret",
                StubTransport {
                    db: &self.db,
                    calls: self.calls.clone(),
                    response,
                },
            )
        }
    }

    fn success(usage: Option<ObservedUsage>) -> Result<TransportResponse, TransportFailure> {
        Ok(TransportResponse {
            body: serde_json::json!({"content": [{"text": "stubbed"}]}),
            upstream_request_id: Some("upstream-1".into()),
            usage,
        })
    }

    #[tokio::test]
    async fn reservation_precedes_transport_and_success_settles_exactly_once() {
        let fixture = Fixture::new(100_000, 100_000);
        let gateway = fixture.gateway(success(Some(ObservedUsage {
            input_tokens: 100,
            cached_input_tokens: 0,
            output_tokens: 10,
        })));
        let request = fixture.request(&gateway, "request-1");

        let result = gateway.forward(request.clone(), NOW).await.unwrap();
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.reservation.status, "settled");
        assert!(result.reservation.observed_micro_usd.is_some());
        assert!(
            result.reservation.observed_micro_usd.unwrap() < result.reservation.reserved_micro_usd
        );
        assert_eq!(fixture.db.provider_spend_row_count("request-1"), 1);

        let replay = gateway.forward(request, NOW).await.unwrap();
        assert!(replay.reservation.replayed);
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.db.provider_spend_row_count("request-1"), 1);

        let mut conflict = fixture.request(&gateway, "request-1");
        conflict.body["messages"][0]["content"] = serde_json::json!("bounded stub requesu");
        assert!(matches!(
            gateway.forward(conflict, NOW).await.unwrap_err(),
            GatewayError::Reservation(_)
        ));
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_and_missing_usage_hold_the_full_reservation() {
        let timed = Fixture::new(100_000, 100_000);
        let timeout_gateway = timed.gateway(Err(TransportFailure {
            kind: TransportFailureKind::Timeout,
            upstream_request_id: None,
            message: "stub timeout".into(),
        }));
        let error = timeout_gateway
            .forward(timed.request(&timeout_gateway, "timeout"), NOW)
            .await
            .unwrap_err();
        assert!(matches!(error, GatewayError::Transport(_)));
        let timeout = timed.db.get_provider_reservation("timeout").unwrap();
        assert_eq!(timeout.status, "unresolved");
        assert_eq!(timeout.observed_micro_usd, None);

        let missing = Fixture::new(100_000, 100_000);
        let missing_gateway = missing.gateway(success(None));
        let result = missing_gateway
            .forward(missing.request(&missing_gateway, "missing"), NOW)
            .await
            .unwrap();
        assert_eq!(result.reservation.status, "unresolved");
        assert_eq!(missing.db.provider_spend_row_count("missing"), 0);
        let released = missing
            .db
            .release_provider_request("missing", "supplier report confirms no charge", NOW + 1)
            .unwrap();
        assert_eq!(released.status, "released");
    }

    #[tokio::test]
    async fn unresolved_usage_can_be_reconciled_and_replay_is_a_noop() {
        let fixture = Fixture::new(100_000, 100_000);
        let gateway = fixture.gateway(success(None));
        gateway
            .forward(fixture.request(&gateway, "later"), NOW)
            .await
            .unwrap();

        let settled = fixture
            .db
            .settle_provider_request("later", 123, Some("report-1"), NOW + 1)
            .unwrap();
        assert_eq!(settled.status, "settled");
        assert_eq!(settled.observed_micro_usd, Some(123));
        let replay = fixture
            .db
            .settle_provider_request("later", 123, Some("report-1"), NOW + 2)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(fixture.db.provider_spend_row_count("later"), 1);

        let contradiction = fixture
            .db
            .settle_provider_request("later", 124, Some("report-2"), NOW + 3)
            .unwrap_err();
        assert!(contradiction.contains("mismatch"));
        assert_eq!(
            fixture.db.get_provider_reservation("later").unwrap().status,
            "mismatch"
        );
    }

    #[tokio::test]
    async fn scope_expiry_tampering_and_unbounded_forms_never_reach_transport() {
        let fixture = Fixture::new(100_000, 100_000);
        let gateway = fixture.gateway(success(None));

        let mut wrong_tenant = fixture.request(&gateway, "wrong-tenant");
        wrong_tenant.tenant_id = "tenant-2".into();
        assert_eq!(
            gateway.forward(wrong_tenant, NOW).await.unwrap_err(),
            GatewayError::ScopeMismatch
        );

        let expired = fixture.request(&gateway, "expired");
        assert_eq!(
            gateway.forward(expired, NOW + 60_000).await.unwrap_err(),
            GatewayError::ExpiredCapability
        );

        let mut tampered = fixture.request(&gateway, "tampered");
        tampered.capability.0.push('x');
        assert_eq!(
            gateway.forward(tampered, NOW).await.unwrap_err(),
            GatewayError::InvalidCapability
        );

        let mut tools = fixture.request(&gateway, "tools");
        tools.body["tools"] = serde_json::json!([{"name": "Bash"}]);
        assert!(matches!(
            gateway.forward(tools, NOW).await.unwrap_err(),
            GatewayError::UnboundedRequest(_)
        ));

        let mut invalid_stream = fixture.request(&gateway, "invalid-stream");
        invalid_stream.body["stream"] = serde_json::json!("yes");
        assert!(matches!(
            gateway.forward(invalid_stream, NOW).await.unwrap_err(),
            GatewayError::UnboundedRequest(_)
        ));

        let mut body_model = fixture.request(&gateway, "body-model");
        body_model.body["model"] = serde_json::json!("claude-other");
        assert!(matches!(
            gateway.forward(body_model, NOW).await.unwrap_err(),
            GatewayError::UnboundedRequest(_)
        ));

        let mut body_max = fixture.request(&gateway, "body-max");
        body_max.body["max_tokens"] = serde_json::json!(1_000_000);
        assert!(matches!(
            gateway.forward(body_max, NOW).await.unwrap_err(),
            GatewayError::UnboundedRequest(_)
        ));
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn bounded_cli_tool_definitions_are_admitted() {
        let fixture = Fixture::new(100_000, 100_000);
        let gateway = fixture.gateway(success(Some(ObservedUsage {
            input_tokens: 100,
            cached_input_tokens: 0,
            output_tokens: 10,
        })));
        let mut request = fixture.request(&gateway, "bounded-tools");
        request.body["tools"] = serde_json::json!([
            {
                "name": "Read",
                "description": "Read a file inside the sandbox",
                "input_schema": {"type": "object", "properties": {}}
            },
            {
                "name": "Bash",
                "input_schema": {"type": "object", "properties": {}}
            }
        ]);

        let outcome = gateway.forward(request, NOW).await.unwrap();
        assert_eq!(outcome.reservation.status, "settled");
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn tool_definition_bounds_reject_ambiguous_and_excessive_forms() {
        let non_array = serde_json::json!({"name": "Read"});
        assert!(validate_tools(Some(&non_array)).is_err());

        let duplicate_names = serde_json::json!([
            {"name": "Read", "input_schema": {}},
            {"name": "Read", "input_schema": {}}
        ]);
        assert!(validate_tools(Some(&duplicate_names)).is_err());

        let excessive = Value::Array(
            (0..65)
                .map(|index| {
                    serde_json::json!({
                        "name": format!("Tool{index}"),
                        "input_schema": {}
                    })
                })
                .collect(),
        );
        assert!(validate_tools(Some(&excessive)).is_err());
    }

    #[tokio::test]
    async fn measured_cli_stream_shape_is_bounded_and_settled() {
        let fixture = Fixture::new(100_000, 100_000);
        let gateway = fixture.gateway(success(Some(ObservedUsage {
            input_tokens: 100,
            cached_input_tokens: 0,
            output_tokens: 10,
        })));
        let mut request = fixture.request(&gateway, "measured-cli-stream");
        request.body["stream"] = serde_json::json!(true);
        request.body["tools"] = serde_json::json!([]);

        let outcome = gateway.forward(request, NOW).await.unwrap();
        assert_eq!(outcome.reservation.status, "settled");
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_reservations_cannot_overdraw_capacity() {
        let probe = Fixture::new(100_000, 100_000);
        let probe_gateway = probe.gateway(success(None));
        let request = probe.request(&probe_gateway, "probe");
        let body_len = serde_json::to_vec(&request.body).unwrap().len() as i64;
        let rate = probe
            .db
            .gateway_model_rate("auth-1", SUPPORTED_PROVIDER, MODEL, NOW)
            .unwrap();
        let one_request = upper_bound_cost(
            body_len,
            request.max_output_tokens,
            rate.input_micros_per_1k,
            rate.output_micros_per_1k,
        )
        .unwrap();

        let exact = Fixture::new(one_request, one_request);
        let barrier = std::sync::Barrier::new(2);
        let (left, right) = std::thread::scope(|scope| {
            let left = scope.spawn(|| {
                barrier.wait();
                exact.db.reserve_provider_request(
                    &exact.claims,
                    "concurrent-left",
                    "sha256:left",
                    one_request,
                    NOW,
                )
            });
            let right = scope.spawn(|| {
                barrier.wait();
                exact.db.reserve_provider_request(
                    &exact.claims,
                    "concurrent-right",
                    "sha256:right",
                    one_request,
                    NOW,
                )
            });
            (left.join().unwrap(), right.join().unwrap())
        });
        assert_ne!(left.is_ok(), right.is_ok());
        assert_eq!(
            ["concurrent-left", "concurrent-right"]
                .iter()
                .filter(|key| exact.db.get_provider_reservation(key).is_some())
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn authorization_and_supplier_capacity_are_atomic_limits() {
        let fixture = Fixture::new(100_000, 100_000);
        let gateway = fixture.gateway(success(None));
        let first = fixture.request(&gateway, "first");
        let body_len = serde_json::to_vec(&first.body).unwrap().len() as i64;
        let rate = fixture
            .db
            .gateway_model_rate("auth-1", SUPPORTED_PROVIDER, MODEL, NOW)
            .unwrap();
        let one_request = upper_bound_cost(
            body_len,
            first.max_output_tokens,
            rate.input_micros_per_1k,
            rate.output_micros_per_1k,
        )
        .unwrap();

        // Replace this fixture's generous limits with a second, exact fixture
        // so one unresolved request consumes the entire authorization and
        // supplier capacity.
        let exact = Fixture::new(one_request, one_request);
        let exact_gateway = exact.gateway(success(None));
        exact_gateway
            .forward(exact.request(&exact_gateway, "first"), NOW)
            .await
            .unwrap();
        let error = exact_gateway
            .forward(exact.request(&exact_gateway, "second"), NOW)
            .await
            .unwrap_err();
        assert!(matches!(error, GatewayError::Reservation(_)));
        assert_eq!(exact.calls.load(Ordering::SeqCst), 1);
        assert!(exact.db.get_provider_reservation("second").is_none());
    }

    #[tokio::test]
    async fn a_provably_unsent_request_releases_capacity() {
        let fixture = Fixture::new(100_000, 100_000);
        let gateway = fixture.gateway(Err(TransportFailure {
            kind: TransportFailureKind::NotSent,
            upstream_request_id: None,
            message: "connection could not be opened".into(),
        }));
        let error = gateway
            .forward(fixture.request(&gateway, "not-sent"), NOW)
            .await
            .unwrap_err();
        assert!(matches!(error, GatewayError::Transport(_)));
        assert_eq!(
            fixture
                .db
                .get_provider_reservation("not-sent")
                .unwrap()
                .status,
            "released"
        );
    }

    #[tokio::test]
    async fn supplier_secret_is_never_returned() {
        let fixture = Fixture::new(100_000, 100_000);
        let gateway = fixture.gateway(Ok(TransportResponse {
            body: serde_json::json!({"bad": "supplier-secret"}),
            upstream_request_id: Some("bad-upstream".into()),
            usage: None,
        }));
        assert_eq!(
            gateway
                .forward(fixture.request(&gateway, "secret"), NOW)
                .await
                .unwrap_err(),
            GatewayError::CredentialExposure
        );
        assert_eq!(
            fixture
                .db
                .get_provider_reservation("secret")
                .unwrap()
                .status,
            "unresolved"
        );
        assert_eq!(
            format!("{:?}", fixture.request(&gateway, "debug").capability),
            "SignedCapability([REDACTED])"
        );
    }
}

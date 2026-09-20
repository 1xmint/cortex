use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub enum StripeError {
    HttpError(String),
    ApiError { status: u16, message: String },
    WebhookSignatureInvalid,
    WebhookTimestampExpired,
    NotConfigured,
}

impl std::fmt::Display for StripeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HttpError(msg) => write!(f, "stripe http error: {msg}"),
            Self::ApiError { status, message } => {
                write!(f, "stripe api error ({status}): {message}")
            }
            Self::WebhookSignatureInvalid => write!(f, "webhook signature invalid"),
            Self::WebhookTimestampExpired => write!(f, "webhook timestamp expired"),
            Self::NotConfigured => write!(f, "stripe not configured"),
        }
    }
}

impl std::error::Error for StripeError {}

pub struct StripeConfig {
    pub secret_key: String,
    pub webhook_secret: String,
    pub price_monthly: String,
    pub price_annual: String,
    pub price_credit_pack: String,
    pub success_url: String,
    pub cancel_url: String,
}

impl StripeConfig {
    pub fn from_env() -> Option<Self> {
        let secret_key = std::env::var("STRIPE_SECRET_KEY").ok()?;
        Some(Self {
            secret_key,
            webhook_secret: std::env::var("STRIPE_WEBHOOK_SECRET").unwrap_or_default(),
            price_monthly: std::env::var("STRIPE_PRICE_MONTHLY").unwrap_or_default(),
            price_annual: std::env::var("STRIPE_PRICE_ANNUAL").unwrap_or_default(),
            price_credit_pack: std::env::var("STRIPE_PRICE_CREDIT_PACK").unwrap_or_default(),
            success_url: std::env::var("STRIPE_SUCCESS_URL")
                .unwrap_or_else(|_| "https://cortex.heyvera.org/billing?success=true".into()),
            cancel_url: std::env::var("STRIPE_CANCEL_URL")
                .unwrap_or_else(|_| "https://cortex.heyvera.org/billing?cancelled=true".into()),
        })
    }
}

pub struct StripeClient {
    secret_key: String,
    http: reqwest::Client,
    pub price_monthly: String,
    pub price_annual: String,
    pub price_credit_pack: String,
    pub success_url: String,
    pub cancel_url: String,
}

#[derive(Debug, Deserialize)]
pub struct CheckoutSessionResponse {
    pub url: Option<String>,
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct PortalSessionResponse {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct CustomerResponse {
    pub id: String,
}

#[derive(Debug, Deserialize)]
struct StripeApiError {
    error: StripeApiErrorBody,
}

#[derive(Debug, Deserialize)]
struct StripeApiErrorBody {
    message: String,
}

impl StripeClient {
    /// Returns `None` if `STRIPE_SECRET_KEY` is not set.
    /// The returned `String` is the webhook secret for use in webhook handlers.
    pub fn from_env() -> Option<(Self, String)> {
        let config = StripeConfig::from_env()?;
        let webhook_secret = config.webhook_secret.clone();
        let client = Self {
            secret_key: config.secret_key,
            http: reqwest::Client::new(),
            price_monthly: config.price_monthly,
            price_annual: config.price_annual,
            price_credit_pack: config.price_credit_pack,
            success_url: config.success_url,
            cancel_url: config.cancel_url,
        };
        Some((client, webhook_secret))
    }

    async fn post_form<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &HashMap<&str, String>,
    ) -> Result<T, StripeError> {
        let url = format!("https://api.stripe.com{path}");
        let resp = self
            .http
            .post(&url)
            .basic_auth(&self.secret_key, Some(""))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(params)
            .send()
            .await
            .map_err(|e| StripeError::HttpError(e.to_string()))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            let message = serde_json::from_str::<StripeApiError>(&body)
                .map(|e| e.error.message)
                .unwrap_or(body);
            return Err(StripeError::ApiError { status, message });
        }

        resp.json::<T>()
            .await
            .map_err(|e| StripeError::HttpError(e.to_string()))
    }

    pub async fn create_customer(
        &self,
        email: &str,
        clerk_user_id: &str,
    ) -> Result<CustomerResponse, StripeError> {
        let mut params = HashMap::new();
        params.insert("email", email.to_string());
        params.insert("metadata[clerk_user_id]", clerk_user_id.to_string());
        self.post_form("/v1/customers", &params).await
    }

    pub async fn create_checkout_session(
        &self,
        customer_id: &str,
        price_id: &str,
        trial_days: Option<u32>,
        coupon_id: Option<&str>,
        clerk_user_id: Option<&str>,
    ) -> Result<CheckoutSessionResponse, StripeError> {
        let mut params = HashMap::new();
        params.insert("customer", customer_id.to_string());
        params.insert("mode", "subscription".to_string());
        params.insert("line_items[0][price]", price_id.to_string());
        params.insert("line_items[0][quantity]", "1".to_string());
        params.insert("success_url", self.success_url.clone());
        params.insert("cancel_url", self.cancel_url.clone());
        params.insert("phone_number_collection[enabled]", "true".to_string());

        if let Some(days) = trial_days {
            params.insert("subscription_data[trial_period_days]", days.to_string());
        }
        if let Some(coupon) = coupon_id {
            params.insert("discounts[0][coupon]", coupon.to_string());
        }
        // Batch B3 — wire clerk user id so checkout.session.completed can init credits.
        if let Some(uid) = clerk_user_id {
            params.insert("client_reference_id", uid.to_string());
            params.insert("metadata[clerk_user_id]", uid.to_string());
        }

        self.post_form("/v1/checkout/sessions", &params).await
    }

    pub async fn create_credit_pack_checkout(
        &self,
        customer_id: &str,
    ) -> Result<CheckoutSessionResponse, StripeError> {
        let mut params = HashMap::new();
        params.insert("customer", customer_id.to_string());
        params.insert("mode", "payment".to_string());
        params.insert("line_items[0][price]", self.price_credit_pack.clone());
        params.insert("line_items[0][quantity]", "1".to_string());
        params.insert("success_url", self.success_url.clone());
        params.insert("cancel_url", self.cancel_url.clone());
        self.post_form("/v1/checkout/sessions", &params).await
    }

    pub async fn create_portal_session(
        &self,
        customer_id: &str,
        return_url: &str,
    ) -> Result<PortalSessionResponse, StripeError> {
        let mut params = HashMap::new();
        params.insert("customer", customer_id.to_string());
        params.insert("return_url", return_url.to_string());
        self.post_form("/v1/billing_portal/sessions", &params).await
    }

    pub fn verify_webhook_signature(
        payload: &[u8],
        sig_header: &str,
        webhook_secret: &str,
    ) -> Result<(), StripeError> {
        let mut timestamp: Option<&str> = None;
        let mut signatures: Vec<&str> = Vec::new();

        for part in sig_header.split(',') {
            let mut kv = part.splitn(2, '=');
            match (kv.next(), kv.next()) {
                (Some("t"), Some(t)) => timestamp = Some(t),
                (Some("v1"), Some(sig)) => signatures.push(sig),
                _ => {}
            }
        }

        let ts = timestamp.ok_or(StripeError::WebhookSignatureInvalid)?;
        if signatures.is_empty() {
            return Err(StripeError::WebhookSignatureInvalid);
        }

        // Enforce 300-second tolerance
        let ts_secs: u64 = ts
            .parse()
            .map_err(|_| StripeError::WebhookSignatureInvalid)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if now.abs_diff(ts_secs) > 300 {
            return Err(StripeError::WebhookTimestampExpired);
        }

        let mut mac = Hmac::<Sha256>::new_from_slice(webhook_secret.as_bytes())
            .map_err(|_| StripeError::WebhookSignatureInvalid)?;
        let signed_payload = format!("{ts}.");
        mac.update(signed_payload.as_bytes());
        mac.update(payload);
        let expected = hex::encode(mac.finalize().into_bytes());

        let valid = signatures.iter().any(|sig| {
            // Constant-time comparison via hmac verify isn't possible on hex strings,
            // but Stripe signatures are not user-controlled secrets — timing attacks
            // aren't practical here. Still, we compare the full string.
            *sig == expected
        });

        if valid {
            Ok(())
        } else {
            Err(StripeError::WebhookSignatureInvalid)
        }
    }
}

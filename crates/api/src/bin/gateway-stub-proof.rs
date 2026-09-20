use std::net::SocketAddr;
use std::path::PathBuf;

use cortex_api::db::{Database, SpendAuthorization};
use cortex_api::provider_gateway::{sign_capability, GatewayCapability};

const AUTHORIZATION_ID: &str = "gateway-cli-proof-auth";
const ATTEMPT_ID: &str = "attempt-rust-listener-proof";
const MODEL: &str = "claude-sonnet-4-6";
const SIGNING_KEY: &[u8] = b"gateway-cli-proof-signing-key-32-bytes";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("CORTEX_GATEWAY_CLI_PROOF").as_deref() == Ok("1"),
        "the gateway proof binary requires CORTEX_GATEWAY_CLI_PROOF=1"
    );
    let token_path = PathBuf::from(std::env::var("CORTEX_GATEWAY_PROOF_TOKEN_PATH")?);
    let db_path = PathBuf::from(std::env::var("CORTEX_GATEWAY_PROOF_DB_PATH")?);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let expires_at_ms = now_ms + 10 * 60 * 1_000;

    let db = Database::open(&db_path);
    let price_list_id = db
        .active_price_list()
        .ok_or_else(|| anyhow::anyhow!("the proof database has no active price list"))?
        .id;
    db.set_supplier_capacity("claude", 1_000_000, now_ms)
        .map_err(anyhow::Error::msg)?;
    db.create_spend_authorization(
        &SpendAuthorization {
            id: AUTHORIZATION_ID.into(),
            user_id: "gateway-cli-proof-tenant".into(),
            run_id: "gateway-cli-proof-run".into(),
            attempt_id: ATTEMPT_ID.into(),
            provider: "claude".into(),
            model: MODEL.into(),
            price_list_id,
            max_micro_usd: 1_000_000,
            expires_at_ms,
        },
        now_ms,
    )
    .map_err(anyhow::Error::msg)?;
    let capability = GatewayCapability::new(
        AUTHORIZATION_ID,
        "gateway-cli-proof-tenant",
        "gateway-cli-proof-run",
        ATTEMPT_ID,
        MODEL,
        expires_at_ms,
    );
    let token = sign_capability(SIGNING_KEY, &capability).map_err(anyhow::Error::msg)?;
    std::fs::write(&token_path, token.expose())?;

    let app = cortex_api::build_gateway_cli_proof_router(
        db,
        SIGNING_KEY.to_vec(),
        AUTHORIZATION_ID.into(),
    );
    let address = SocketAddr::from(([127, 0, 0, 1], 18_080));
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

//! Durable supplier-spend controls used by the private provider gateway.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendAuthorization {
    pub id: String,
    pub user_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub provider: String,
    pub model: String,
    pub price_list_id: String,
    pub max_micro_usd: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReservation {
    pub id: String,
    pub request_key: String,
    pub request_digest: String,
    pub authorization_id: String,
    pub reserved_micro_usd: i64,
    pub observed_micro_usd: Option<i64>,
    pub status: String,
    pub upstream_request_id: Option<String>,
    pub terminal_reason: Option<String>,
    pub replayed: bool,
}

fn read_reservation(conn: &Connection, request_key: &str) -> Option<ProviderReservation> {
    conn.query_row(
        "SELECT id, request_key, request_digest, authorization_id, reserved_micro_usd,
                observed_micro_usd, status, upstream_request_id, terminal_reason
         FROM provider_request_reservations WHERE request_key = ?1",
        params![request_key],
        |row| {
            Ok(ProviderReservation {
                id: row.get(0)?,
                request_key: row.get(1)?,
                request_digest: row.get(2)?,
                authorization_id: row.get(3)?,
                reserved_micro_usd: row.get(4)?,
                observed_micro_usd: row.get(5)?,
                status: row.get(6)?,
                upstream_request_id: row.get(7)?,
                terminal_reason: row.get(8)?,
                replayed: false,
            })
        },
    )
    .ok()
}

impl Database {
    pub fn get_provider_reservation(&self, request_key: &str) -> Option<ProviderReservation> {
        let conn = self.conn();
        read_reservation(&conn, request_key)
    }

    pub fn set_supplier_capacity(
        &self,
        provider: &str,
        funded_micro_usd: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        if funded_micro_usd < 0 {
            return Err("supplier capacity cannot be negative".into());
        }
        let conn = self.conn();
        conn.execute(
            "INSERT INTO supplier_capacities(provider, funded_micro_usd, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(provider) DO UPDATE SET
                funded_micro_usd = excluded.funded_micro_usd,
                updated_at = excluded.updated_at",
            params![provider, funded_micro_usd, now_ms],
        )
        .map_err(|e| format!("failed to set supplier capacity: {e}"))?;
        Ok(())
    }

    pub fn create_spend_authorization(
        &self,
        authorization: &SpendAuthorization,
        now_ms: i64,
    ) -> Result<(), String> {
        if authorization.max_micro_usd <= 0 || authorization.expires_at_ms <= now_ms {
            return Err("spend authorization must be positive and unexpired".into());
        }
        let conn = self.conn();
        conn.execute(
            "INSERT OR IGNORE INTO provider_spend_authorizations
                (id, user_id, run_id, attempt_id, provider, model, price_list_id,
                 max_micro_usd, expires_at, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', ?10)",
            params![
                authorization.id,
                authorization.user_id,
                authorization.run_id,
                authorization.attempt_id,
                authorization.provider,
                authorization.model,
                authorization.price_list_id,
                authorization.max_micro_usd,
                authorization.expires_at_ms,
                now_ms,
            ],
        )
        .map_err(|e| format!("failed to create spend authorization: {e}"))?;
        let exact: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provider_spend_authorizations
                 WHERE id = ?1 AND user_id = ?2 AND run_id = ?3 AND attempt_id = ?4
                   AND provider = ?5 AND model = ?6 AND price_list_id = ?7
                   AND max_micro_usd = ?8 AND expires_at = ?9 AND status = 'active'",
                params![
                    authorization.id,
                    authorization.user_id,
                    authorization.run_id,
                    authorization.attempt_id,
                    authorization.provider,
                    authorization.model,
                    authorization.price_list_id,
                    authorization.max_micro_usd,
                    authorization.expires_at_ms,
                ],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to verify spend authorization: {e}"))?;
        if exact != 1 {
            return Err("spend authorization replay conflicts with its original scope".into());
        }
        Ok(())
    }

    pub fn gateway_model_rate(
        &self,
        authorization_id: &str,
        provider: &str,
        model: &str,
        now_ms: i64,
    ) -> Option<crate::pricing::ModelPrice> {
        let conn = self.conn();
        conn.query_row(
            "SELECT m.provider, m.model_id, m.input_micros_per_1k,
                    m.output_micros_per_1k, m.cache_read_bp, m.context_window,
                    m.capability_class
             FROM provider_spend_authorizations a
             JOIN price_list_models m ON m.price_list_id = a.price_list_id
             WHERE a.id = ?1 AND a.provider = ?2 AND a.model = ?3
               AND a.status = 'active' AND a.expires_at > ?4
               AND m.provider = a.provider AND m.model_id = a.model",
            params![authorization_id, provider, model, now_ms],
            |row| {
                Ok(crate::pricing::ModelPrice {
                    provider: row.get(0)?,
                    model_id: row.get(1)?,
                    input_micros_per_1k: row.get(2)?,
                    output_micros_per_1k: row.get(3)?,
                    cache_read_bp: row.get(4)?,
                    context_window: row.get(5)?,
                    capability_class: row.get(6)?,
                })
            },
        )
        .ok()
    }

    pub fn reserve_provider_request(
        &self,
        claims: &crate::provider_gateway::GatewayCapability,
        request_key: &str,
        request_digest: &str,
        reserved_micro_usd: i64,
        now_ms: i64,
    ) -> Result<ProviderReservation, String> {
        if reserved_micro_usd <= 0
            || request_key.trim().is_empty()
            || request_digest.trim().is_empty()
        {
            return Err("reservation amount, request key, and request digest are required".into());
        }
        let mut conn = self.conn();
        let tx = conn
            .transaction()
            .map_err(|e| format!("failed to begin reservation transaction: {e}"))?;

        if let Some(mut existing) = read_reservation(&tx, request_key) {
            if existing.authorization_id != claims.authorization_id
                || existing.request_digest != request_digest
                || existing.reserved_micro_usd != reserved_micro_usd
            {
                return Err("request key replay conflicts with its original reservation".into());
            }
            existing.replayed = true;
            return Ok(existing);
        }

        let authorization: SpendAuthorization = tx
            .query_row(
                "SELECT id, user_id, run_id, attempt_id, provider, model,
                        price_list_id, max_micro_usd, expires_at
                 FROM provider_spend_authorizations
                 WHERE id = ?1 AND status = 'active' AND expires_at > ?2",
                params![claims.authorization_id, now_ms],
                |row| {
                    Ok(SpendAuthorization {
                        id: row.get(0)?,
                        user_id: row.get(1)?,
                        run_id: row.get(2)?,
                        attempt_id: row.get(3)?,
                        provider: row.get(4)?,
                        model: row.get(5)?,
                        price_list_id: row.get(6)?,
                        max_micro_usd: row.get(7)?,
                        expires_at_ms: row.get(8)?,
                    })
                },
            )
            .map_err(|_| "spend authorization is missing, revoked, or expired".to_string())?;
        if authorization.user_id != claims.tenant_id
            || authorization.run_id != claims.run_id
            || authorization.attempt_id != claims.attempt_id
            || authorization.provider != claims.provider
            || authorization.model != claims.model
            || authorization.expires_at_ms != claims.expires_at_ms
        {
            return Err("capability does not match its durable spending authorization".into());
        }

        let authorized_used: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(
                    CASE WHEN observed_micro_usd > reserved_micro_usd THEN observed_micro_usd
                         WHEN status = 'settled' THEN observed_micro_usd
                         ELSE reserved_micro_usd END
                 ), 0)
                 FROM provider_request_reservations
                 WHERE authorization_id = ?1 AND status != 'released'",
                params![authorization.id],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to read authorization exposure: {e}"))?;
        let requested_authorization = authorized_used
            .checked_add(reserved_micro_usd)
            .ok_or("authorization exposure overflow")?;
        if requested_authorization > authorization.max_micro_usd {
            return Err(format!(
                "authorization exhausted: need {requested_authorization}, max {}",
                authorization.max_micro_usd
            ));
        }

        let funded: i64 = tx
            .query_row(
                "SELECT funded_micro_usd FROM supplier_capacities WHERE provider = ?1",
                params![authorization.provider],
                |row| row.get(0),
            )
            .map_err(|_| "supplier capacity is not funded".to_string())?;
        let supplier_used: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(
                    CASE WHEN observed_micro_usd > reserved_micro_usd THEN observed_micro_usd
                         WHEN status = 'settled' THEN observed_micro_usd
                         ELSE reserved_micro_usd END
                 ), 0)
                 FROM provider_request_reservations
                 WHERE provider = ?1 AND status != 'released'",
                params![authorization.provider],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to read supplier exposure: {e}"))?;
        let requested_supplier = supplier_used
            .checked_add(reserved_micro_usd)
            .ok_or("supplier exposure overflow")?;
        if requested_supplier > funded {
            return Err(format!(
                "supplier capacity exhausted: need {requested_supplier}, funded {funded}"
            ));
        }

        let id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO provider_request_reservations
                (id, request_key, request_digest, authorization_id, user_id, run_id, attempt_id,
                 provider, model, price_list_id, reserved_micro_usd, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'reserved', ?12)",
            params![
                id,
                request_key,
                request_digest,
                authorization.id,
                authorization.user_id,
                authorization.run_id,
                authorization.attempt_id,
                authorization.provider,
                authorization.model,
                authorization.price_list_id,
                reserved_micro_usd,
                now_ms,
            ],
        )
        .map_err(|e| format!("failed to persist supplier reservation: {e}"))?;
        tx.commit()
            .map_err(|e| format!("failed to commit supplier reservation: {e}"))?;
        read_reservation(&conn, request_key)
            .ok_or_else(|| "committed reservation could not be read".to_string())
    }

    pub fn mark_provider_request_unresolved(
        &self,
        request_key: &str,
        upstream_request_id: Option<&str>,
        reason: &str,
        now_ms: i64,
    ) -> Result<ProviderReservation, String> {
        let conn = self.conn();
        conn.execute(
            "UPDATE provider_request_reservations
             SET status = 'unresolved', upstream_request_id = COALESCE(upstream_request_id, ?1),
                 terminal_reason = ?2, reconciled_at = ?3
             WHERE request_key = ?4 AND status = 'reserved'",
            params![upstream_request_id, reason, now_ms, request_key],
        )
        .map_err(|e| format!("failed to preserve unresolved reservation: {e}"))?;
        read_reservation(&conn, request_key).ok_or_else(|| "reservation does not exist".to_string())
    }

    pub fn release_provider_request(
        &self,
        request_key: &str,
        reason: &str,
        now_ms: i64,
    ) -> Result<ProviderReservation, String> {
        let conn = self.conn();
        conn.execute(
            "UPDATE provider_request_reservations
             SET status = 'released', terminal_reason = ?1, reconciled_at = ?2
             WHERE request_key = ?3 AND status IN ('reserved', 'unresolved')",
            params![reason, now_ms, request_key],
        )
        .map_err(|e| format!("failed to release supplier reservation: {e}"))?;
        read_reservation(&conn, request_key).ok_or_else(|| "reservation does not exist".to_string())
    }

    pub fn settle_provider_request(
        &self,
        request_key: &str,
        observed_micro_usd: i64,
        upstream_request_id: Option<&str>,
        now_ms: i64,
    ) -> Result<ProviderReservation, String> {
        if observed_micro_usd < 0 {
            return Err("observed supplier cost cannot be negative".into());
        }
        let mut conn = self.conn();
        let tx = conn
            .transaction()
            .map_err(|e| format!("failed to begin reconciliation transaction: {e}"))?;
        let Some(current) = read_reservation(&tx, request_key) else {
            return Err("reservation does not exist".into());
        };
        if current.status == "settled" && current.observed_micro_usd == Some(observed_micro_usd) {
            let mut replay = current;
            replay.replayed = true;
            return Ok(replay);
        }
        if current.status == "settled" || current.status == "released" {
            tx.execute(
                "UPDATE provider_request_reservations
                 SET status = 'mismatch', terminal_reason = ?1, reconciled_at = ?2
                 WHERE request_key = ?3",
                params![
                    format!(
                        "contradictory reconciliation: existing {:?}, observed {observed_micro_usd}",
                        current.observed_micro_usd
                    ),
                    now_ms,
                    request_key
                ],
            )
            .map_err(|e| format!("failed to record reconciliation mismatch: {e}"))?;
            tx.commit()
                .map_err(|e| format!("failed to commit reconciliation mismatch: {e}"))?;
            return Err("contradictory reconciliation recorded as mismatch".into());
        }
        if observed_micro_usd > current.reserved_micro_usd {
            tx.execute(
                "UPDATE provider_request_reservations
                 SET status = 'mismatch', observed_micro_usd = ?1,
                     upstream_request_id = COALESCE(upstream_request_id, ?2),
                     terminal_reason = 'observed cost exceeded reservation', reconciled_at = ?3
                 WHERE request_key = ?4",
                params![observed_micro_usd, upstream_request_id, now_ms, request_key],
            )
            .map_err(|e| format!("failed to record over-reservation mismatch: {e}"))?;
            tx.commit()
                .map_err(|e| format!("failed to commit over-reservation mismatch: {e}"))?;
            return Err("observed supplier cost exceeded the durable reservation".into());
        }

        tx.execute(
            "UPDATE provider_request_reservations
             SET status = 'settled', observed_micro_usd = ?1,
                 upstream_request_id = COALESCE(upstream_request_id, ?2),
                 terminal_reason = NULL, reconciled_at = ?3
             WHERE request_key = ?4 AND status IN ('reserved', 'unresolved')",
            params![observed_micro_usd, upstream_request_id, now_ms, request_key],
        )
        .map_err(|e| format!("failed to settle supplier reservation: {e}"))?;

        tx.execute(
            "INSERT OR IGNORE INTO provider_spend
                (id, user_id, run_id, step_id, provider, model, cost_type,
                 tokens_in, tokens_out, tokens_cached_in, cost_micro_usd, created_at)
             SELECT 'gateway:' || id, user_id, run_id, attempt_id, provider, model,
                    'gateway_observed', 0, 0, 0, ?1, ?2
             FROM provider_request_reservations WHERE request_key = ?3",
            params![observed_micro_usd, now_ms, request_key],
        )
        .map_err(|e| format!("failed to record observed provider spend: {e}"))?;
        tx.commit()
            .map_err(|e| format!("failed to commit provider reconciliation: {e}"))?;
        read_reservation(&conn, request_key)
            .ok_or_else(|| "settled reservation could not be read".to_string())
    }

    pub fn provider_spend_row_count(&self, request_key: &str) -> i64 {
        let conn = self.conn();
        conn.query_row(
            "SELECT COUNT(*) FROM provider_spend s
             JOIN provider_request_reservations r ON s.id = 'gateway:' || r.id
             WHERE r.request_key = ?1",
            params![request_key],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    #[cfg(feature = "gateway-cli-proof")]
    pub fn provider_authorization_spend_summary(&self, authorization_id: &str) -> (i64, i64) {
        let conn = self.conn();
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN status = 'settled' THEN 1 ELSE 0 END), 0)
             FROM provider_request_reservations WHERE authorization_id = ?1",
            params![authorization_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, 0))
    }
}

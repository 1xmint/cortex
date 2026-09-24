//! Durable supplier-spend controls used by the private provider gateway.

use super::*;

/// A `reserved` row older than this at startup is treated as orphaned: the
/// process that would settle, release, or explicitly mark it unresolved died
/// with the request in flight. The longest upstream call the gateway makes
/// is capped at 600s (`UPSTREAM_TIMEOUT` in `supplier_anthropic.rs` and
/// `supplier_openai.rs`); this adds a generous margin on top so an in-flight
/// request from just before restart is never swept out from under it.
pub const STALE_RESERVATION_AGE_MS: i64 = 15 * 60 * 1000;

/// Share of funded supplier capacity that `reserved` + `unresolved` holds
/// may occupy before operators are warned that stuck holds are crowding out
/// real capacity.
pub const HOLD_CAPACITY_WARN_SHARE: f64 = 0.5;

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

/// One `reserved` or `unresolved` hold, for the admin operator view.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderHoldRow {
    pub request_key: String,
    pub provider: String,
    pub model: String,
    pub amount_micro_usd: i64,
    pub status: String,
    pub age_ms: i64,
    pub terminal_reason: Option<String>,
}

/// Aggregate view of supplier holds for the admin dashboard: how much is
/// tied up `reserved` or `unresolved`, how that compares to funded supplier
/// capacity, and a bounded, oldest-first sample of the actual rows so an
/// operator can see what is stuck without pulling the database open.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderHoldsSummary {
    pub reserved_count: i64,
    pub reserved_total_micro_usd: i64,
    pub unresolved_count: i64,
    pub unresolved_total_micro_usd: i64,
    pub mismatch_count: i64,
    pub oldest_age_ms: Option<i64>,
    pub funded_total_micro_usd: i64,
    pub over_threshold: bool,
    pub rows: Vec<ProviderHoldRow>,
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

/// Error from an admin-initiated hold action: distinguishes an unknown
/// `request_key` (404) from a row that cannot be resolved right now
/// (409), so callers never need to parse the message to pick a status code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminHoldError {
    NotFound,
    Conflict(String),
}

impl std::fmt::Display for AdminHoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdminHoldError::NotFound => write!(f, "provider hold not found"),
            AdminHoldError::Conflict(message) => write!(f, "{message}"),
        }
    }
}

/// Shared reconciliation write path for `settle_provider_request` and
/// `admin_settle_provider_hold`. Takes ownership of an already-open
/// transaction and commits it itself on every path that writes anything
/// (including the "recorded as mismatch" error paths, which persist their
/// write and still return `Err`); the no-op replay path writes nothing and
/// lets the transaction drop, which rolls back trivially. Callers that need
/// a check-then-act guarantee must do their own status check against this
/// same transaction (via `read_reservation(&tx, ...)`) before calling this.
fn settle_within_tx(
    tx: rusqlite::Transaction<'_>,
    request_key: &str,
    observed_micro_usd: i64,
    upstream_request_id: Option<&str>,
    now_ms: i64,
) -> Result<ProviderReservation, String> {
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

    let updated = read_reservation(&tx, request_key)
        .ok_or_else(|| "settled reservation could not be read".to_string())?;
    tx.commit()
        .map_err(|e| format!("failed to commit provider reconciliation: {e}"))?;
    Ok(updated)
}

impl Database {
    pub fn get_provider_reservation(&self, request_key: &str) -> Option<ProviderReservation> {
        let conn = self.conn();
        read_reservation(&conn, request_key)
    }

    /// Record one Zen BYOK reply for analytics only: `cost_type = 'byok'`,
    /// `cost_micro_usd = 0` always, because the customer paid Zen directly
    /// and Cortex paid nothing (D1/A1 in the BYOK plan). Deliberately writes
    /// to `provider_spend` and nothing else on this path -- no
    /// `provider_request_reservations` row, no `provider_spend_authorizations`
    /// row, no `credit_transactions` row.
    ///
    /// `id` is `"byok:{reply_id}"`; `INSERT OR IGNORE` makes a retried insert
    /// for the same reply idempotent rather than double-counted.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_byok_usage(
        &self,
        reply_id: &str,
        user_id: &str,
        conversation_id: Option<&str>,
        model: &str,
        tokens_in: i64,
        tokens_out: i64,
        tokens_cached_in: i64,
        now_ms: i64,
    ) {
        let conn = self.conn();
        conn.execute(
            "INSERT OR IGNORE INTO provider_spend
                (id, user_id, run_id, step_id, provider, model, cost_type,
                 tokens_in, tokens_out, tokens_cached_in, cost_micro_usd, created_at)
             VALUES (?1, ?2, ?3, NULL, 'zen', ?4, 'byok', ?5, ?6, ?7, 0, ?8)",
            params![
                format!("byok:{reply_id}"),
                user_id,
                conversation_id,
                model,
                tokens_in,
                tokens_out,
                tokens_cached_in,
                now_ms,
            ],
        )
        .expect("insert byok provider_spend row");
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

    /// Revokes a single spend authorization by id. Used when a step's lease
    /// is lost (or its gen advances) between issuing the authorization and
    /// sending `ExecuteStep`: the worker was never told to run, so the
    /// authorization must not be left `active` for `reserve_provider_request`
    /// to honor.
    pub fn revoke_spend_authorization(&self, authorization_id: &str) {
        let conn = self.conn();
        if let Err(e) = conn.execute(
            "UPDATE provider_spend_authorizations SET status = 'revoked'
             WHERE id = ?1 AND status = 'active'",
            params![authorization_id],
        ) {
            tracing::error!(
                authorization_id,
                error = %e,
                "failed to revoke spend authorization for an undispatched step"
            );
        }
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
                 WHERE id = ?1 AND status = 'active' AND expires_at > ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM runs r
                       WHERE r.id = provider_spend_authorizations.run_id
                         AND r.status = 'cancelled'
                   )",
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
        let reservation = read_reservation(&conn, request_key)
            .ok_or_else(|| "reservation does not exist".to_string())?;
        drop(conn);
        self.warn_if_holds_over_threshold(now_ms);
        Ok(reservation)
    }

    /// Startup sweep: any non-voice (`chat`, etc.) reservation still
    /// `reserved` at process start and older than `STALE_RESERVATION_AGE_MS`
    /// was left mid-flight by a server restart or a crashed request — mark
    /// it `unresolved` for reconciliation instead of leaving it `reserved`
    /// (and counted against funded capacity) forever. Only rows are aged out
    /// because a genuinely fresh `reserved` row could still be an in-flight
    /// request from just before this process started. Voice reservations are
    /// swept separately by `sweep_stale_voice_reservations` (no age check —
    /// see its doc comment) and are excluded here so this never double-acts
    /// on them.
    ///
    /// This assumes this process is the only server using this database
    /// (`CORTEX_SINGLE_NODE`), same as the voice sweep.
    pub fn sweep_stale_reservations(&self, now_ms: i64) -> Result<usize, String> {
        let conn = self.conn();
        let reason = "server restarted with a non-voice reservation stuck in flight";
        let cutoff_ms = now_ms - STALE_RESERVATION_AGE_MS;
        let swept = conn
            .execute(
                "UPDATE provider_request_reservations
                 SET status = 'unresolved', terminal_reason = ?1, reconciled_at = ?2
                 WHERE status = 'reserved' AND request_key NOT LIKE 'voice:%'
                   AND created_at < ?3",
                params![reason, now_ms, cutoff_ms],
            )
            .map_err(|e| format!("failed to sweep stale reservations: {e}"))?;
        drop(conn);
        Ok(swept)
    }

    /// Startup sweep: any `voice:*` reservation still `reserved` at process
    /// start was left mid-flight by a server restart (the billing task that
    /// owned it, and the sideband drop cleanup that would otherwise mark it
    /// unresolved, both died with the old process) — mark it `unresolved`
    /// for reconciliation instead of leaving it `reserved` forever. Returns
    /// how many rows it swept, for a log line at startup.
    ///
    /// This assumes this process is the only server using this database
    /// (`CORTEX_SINGLE_NODE`): it marks every `reserved` voice row
    /// unresolved unconditionally, without checking whether some other
    /// server process still has that session live.
    pub fn sweep_stale_voice_reservations(&self, now_ms: i64) -> Result<usize, String> {
        let conn = self.conn();
        let reason = "server restarted during live session";
        let swept = conn
            .execute(
                "UPDATE provider_request_reservations
                 SET status = 'unresolved', terminal_reason = ?1, reconciled_at = ?2
                 WHERE status = 'reserved' AND request_key LIKE 'voice:%'",
                params![reason, now_ms],
            )
            .map_err(|e| format!("failed to sweep stale voice reservations: {e}"))?;
        drop(conn);
        Ok(swept)
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
        settle_within_tx(
            tx,
            request_key,
            observed_micro_usd,
            upstream_request_id,
            now_ms,
        )
    }

    /// Same reconciliation as `settle_provider_request`, but for a manual
    /// admin settle: the "is this row still resolvable" check and the write
    /// happen inside one transaction, so a gateway settlement landing between
    /// a separate check-then-act pair can never flip the row to `mismatch`
    /// out from under the admin, or have a stale request replayed and
    /// audited as the admin's own action. Returns the row as it was
    /// immediately before the admin's write (for the audit record) and as it
    /// is after.
    pub fn admin_settle_provider_hold(
        &self,
        request_key: &str,
        observed_micro_usd: i64,
        upstream_request_id: Option<&str>,
        now_ms: i64,
    ) -> Result<(ProviderReservation, ProviderReservation), AdminHoldError> {
        if observed_micro_usd < 0 {
            return Err(AdminHoldError::Conflict(
                "observed supplier cost cannot be negative".into(),
            ));
        }
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(|e| {
            AdminHoldError::Conflict(format!("failed to begin reconciliation transaction: {e}"))
        })?;
        let Some(prior) = read_reservation(&tx, request_key) else {
            return Err(AdminHoldError::NotFound);
        };
        if prior.status != "reserved" && prior.status != "unresolved" {
            return Err(AdminHoldError::Conflict(format!(
                "provider hold is already terminal (status: {})",
                prior.status
            )));
        }
        let updated = settle_within_tx(
            tx,
            request_key,
            observed_micro_usd,
            upstream_request_id,
            now_ms,
        )
        .map_err(AdminHoldError::Conflict)?;
        Ok((prior, updated))
    }

    /// Same one-transaction treatment as `admin_settle_provider_hold`, for a
    /// manual admin release: the resolvability check and the write happen
    /// inside one transaction, and nothing is written unless the release
    /// actually applied. Returns the row as it was immediately before the
    /// admin's write (for the audit record) and as it is after.
    pub fn admin_release_provider_hold(
        &self,
        request_key: &str,
        reason: &str,
        now_ms: i64,
    ) -> Result<(ProviderReservation, ProviderReservation), AdminHoldError> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(|e| {
            AdminHoldError::Conflict(format!("failed to begin release transaction: {e}"))
        })?;
        let Some(prior) = read_reservation(&tx, request_key) else {
            return Err(AdminHoldError::NotFound);
        };
        if prior.status != "reserved" && prior.status != "unresolved" {
            return Err(AdminHoldError::Conflict(format!(
                "provider hold is already terminal (status: {})",
                prior.status
            )));
        }
        let changed = tx
            .execute(
                "UPDATE provider_request_reservations
                 SET status = 'released', terminal_reason = ?1, reconciled_at = ?2
                 WHERE request_key = ?3 AND status IN ('reserved', 'unresolved')",
                params![reason, now_ms, request_key],
            )
            .map_err(|e| {
                AdminHoldError::Conflict(format!("failed to release supplier reservation: {e}"))
            })?;
        let updated = read_reservation(&tx, request_key).ok_or(AdminHoldError::NotFound)?;
        if changed == 0 || updated.status != "released" {
            return Err(AdminHoldError::Conflict(format!(
                "provider hold release did not apply (status: {})",
                updated.status
            )));
        }
        tx.commit()
            .map_err(|e| AdminHoldError::Conflict(format!("failed to commit release: {e}")))?;
        Ok((prior, updated))
    }

    /// Sum of `reserved` + `unresolved` exposure across all providers,
    /// alongside total funded supplier capacity. Used both by the admin
    /// summary endpoint and by `warn_if_holds_over_threshold`.
    fn holds_vs_funded(&self, conn: &Connection) -> (i64, i64) {
        let holds_total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(reserved_micro_usd), 0)
                 FROM provider_request_reservations WHERE status IN ('reserved', 'unresolved')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let funded_total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(funded_micro_usd), 0) FROM supplier_capacities",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        (holds_total, funded_total)
    }

    /// Log a `tracing::warn!` when stuck (`reserved` + `unresolved`) holds
    /// occupy more than `HOLD_CAPACITY_WARN_SHARE` of funded supplier
    /// capacity. Called at startup after the sweeps run, and whenever a row
    /// newly becomes `unresolved`, so an operator sees the warning close to
    /// when it started being true rather than only on the next GET.
    pub fn warn_if_holds_over_threshold(&self, now_ms: i64) {
        let _ = now_ms;
        let conn = self.conn();
        let (holds_total, funded_total) = self.holds_vs_funded(&conn);
        if funded_total > 0
            && (holds_total as f64) >= (funded_total as f64) * HOLD_CAPACITY_WARN_SHARE
        {
            tracing::warn!(
                holds_total_micro_usd = holds_total,
                funded_total_micro_usd = funded_total,
                share = holds_total as f64 / funded_total as f64,
                "provider gateway: reserved+unresolved holds exceed {}% of funded supplier capacity",
                (HOLD_CAPACITY_WARN_SHARE * 100.0) as i64,
            );
        }
    }

    /// Admin operator view: counts and totals for `reserved` and
    /// `unresolved` holds, the mismatch count, the oldest outstanding hold's
    /// age, funded capacity comparison, and a bounded, oldest-first sample of
    /// the rows themselves. Oldest-first because the rows most worth an
    /// operator's attention are the ones that have been stuck longest, not
    /// the ones that just started.
    pub fn provider_holds_summary(&self, now_ms: i64, limit: i64) -> ProviderHoldsSummary {
        let conn = self.conn();
        let limit = limit.clamp(1, 500);

        let (reserved_count, reserved_total_micro_usd, reserved_oldest): (i64, i64, Option<i64>) =
            conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(reserved_micro_usd), 0), MIN(created_at)
                 FROM provider_request_reservations WHERE status = 'reserved'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((0, 0, None));

        let (unresolved_count, unresolved_total_micro_usd, unresolved_oldest): (
            i64,
            i64,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(reserved_micro_usd), 0), MIN(created_at)
                 FROM provider_request_reservations WHERE status = 'unresolved'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((0, 0, None));

        let mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provider_request_reservations WHERE status = 'mismatch'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let oldest_created_at = [reserved_oldest, unresolved_oldest]
            .into_iter()
            .flatten()
            .min();
        let oldest_age_ms = oldest_created_at.map(|created_at| (now_ms - created_at).max(0));

        let (holds_total, funded_total_micro_usd) = self.holds_vs_funded(&conn);
        let over_threshold = funded_total_micro_usd > 0
            && (holds_total as f64) >= (funded_total_micro_usd as f64) * HOLD_CAPACITY_WARN_SHARE;

        let mut rows = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT request_key, provider, model, reserved_micro_usd, status, created_at,
                    terminal_reason
             FROM provider_request_reservations
             WHERE status IN ('reserved', 'unresolved')
             ORDER BY created_at ASC
             LIMIT ?1",
        ) {
            if let Ok(mapped) = stmt.query_map(params![limit], |row| {
                let created_at: i64 = row.get(5)?;
                Ok(ProviderHoldRow {
                    request_key: row.get(0)?,
                    provider: row.get(1)?,
                    model: row.get(2)?,
                    amount_micro_usd: row.get(3)?,
                    status: row.get(4)?,
                    age_ms: (now_ms - created_at).max(0),
                    terminal_reason: row.get(6)?,
                })
            }) {
                rows.extend(mapped.filter_map(|r| r.ok()));
            }
        }

        ProviderHoldsSummary {
            reserved_count,
            reserved_total_micro_usd,
            unresolved_count,
            unresolved_total_micro_usd,
            mismatch_count,
            oldest_age_ms,
            funded_total_micro_usd,
            over_threshold,
            rows,
        }
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

#[cfg(test)]
mod tests {
    use super::super::tests::test_db;
    use super::*;
    use crate::provider_gateway::GatewayCapability;

    const NOW: i64 = 1_800_000_000_000;
    const PROVIDER: &str = "claude";
    const MODEL: &str = "claude-sonnet-5";

    /// A funded provider with an active spend authorization, ready for
    /// `reserve_provider_request`.
    fn fixture(db: &Database, max_micro_usd: i64, funded_micro_usd: i64) -> GatewayCapability {
        fixture_with_expiry(db, max_micro_usd, funded_micro_usd, NOW + 60_000)
    }

    /// Same as `fixture`, but with a caller-chosen `expires_at_ms`, for tests
    /// that reserve well after `NOW` and need an authorization that is still
    /// active at that later timestamp.
    fn fixture_with_expiry(
        db: &Database,
        max_micro_usd: i64,
        funded_micro_usd: i64,
        expires_at_ms: i64,
    ) -> GatewayCapability {
        let price_list_id = db.active_price_list().unwrap().id;
        db.set_supplier_capacity(PROVIDER, funded_micro_usd, NOW)
            .unwrap();
        let authorization = SpendAuthorization {
            id: "auth-1".into(),
            user_id: "tenant-1".into(),
            run_id: "run-1".into(),
            attempt_id: "attempt-1".into(),
            provider: PROVIDER.into(),
            model: MODEL.into(),
            price_list_id,
            max_micro_usd,
            expires_at_ms,
        };
        db.create_spend_authorization(&authorization, NOW).unwrap();
        GatewayCapability::new(
            authorization.id,
            authorization.user_id,
            authorization.run_id,
            authorization.attempt_id,
            authorization.provider,
            authorization.model,
            authorization.expires_at_ms,
        )
    }

    fn reserve(db: &Database, claims: &GatewayCapability, request_key: &str, now_ms: i64) {
        db.reserve_provider_request(claims, request_key, "digest", 1_000, now_ms)
            .unwrap();
    }

    // --- sweep_stale_reservations ---

    #[test]
    fn sweep_marks_old_chat_reserved_rows_unresolved_and_leaves_fresh_rows_alone() {
        let db = test_db();
        // Sweeping happens well after any of these reservations are made, so
        // the authorization must still be active that far out.
        let claims = fixture_with_expiry(
            &db,
            1_000_000,
            1_000_000,
            NOW + 2 * STALE_RESERVATION_AGE_MS,
        );

        // Created well before the staleness cutoff: orphaned by a restart.
        reserve(&db, &claims, "chat:old", NOW);
        // Created at the moment the sweep's cutoff will land on: not older
        // than the cutoff (the sweep's comparison is strict `<`), so it must
        // survive.
        reserve(&db, &claims, "chat:fresh", NOW + STALE_RESERVATION_AGE_MS);
        // A second row created at that exact same cutoff instant, kept
        // distinct from `chat:fresh` so the boundary condition itself is
        // asserted on its own, not just incidentally via the "fresh" row.
        reserve(
            &db,
            &claims,
            "chat:at-cutoff",
            NOW + STALE_RESERVATION_AGE_MS,
        );
        // Voice rows are swept separately and must never be touched here.
        reserve(&db, &claims, "voice:old", NOW);

        // Cutoff lands exactly on the "fresh" rows' `created_at`.
        let sweep_now = NOW + 2 * STALE_RESERVATION_AGE_MS;
        let swept = db.sweep_stale_reservations(sweep_now).unwrap();
        assert_eq!(swept, 1, "only the old, non-voice row should be swept");

        let old = db.get_provider_reservation("chat:old").unwrap();
        assert_eq!(old.status, "unresolved");
        assert!(old.terminal_reason.is_some());

        let fresh = db.get_provider_reservation("chat:fresh").unwrap();
        assert_eq!(
            fresh.status, "reserved",
            "a row younger than the staleness cutoff must be left alone"
        );

        let at_cutoff = db.get_provider_reservation("chat:at-cutoff").unwrap();
        assert_eq!(
            at_cutoff.status, "reserved",
            "a row created exactly at the cutoff is not yet older than it, and must be left alone"
        );

        let voice = db.get_provider_reservation("voice:old").unwrap();
        assert_eq!(
            voice.status, "reserved",
            "voice rows are swept by sweep_stale_voice_reservations, not this sweep"
        );

        // Sweeping again is a no-op: nothing left to sweep.
        assert_eq!(db.sweep_stale_reservations(sweep_now).unwrap(), 0);
    }

    // --- provider_holds_summary ---

    #[test]
    fn get_totals_reflect_reserved_and_unresolved_holds() {
        let db = test_db();
        let claims = fixture(&db, 1_000_000, 1_000_000);

        reserve(&db, &claims, "chat:a", NOW);
        reserve(&db, &claims, "chat:b", NOW + 10);
        db.mark_provider_request_unresolved("chat:b", None, "timed out", NOW + 20)
            .unwrap();

        let summary = db.provider_holds_summary(NOW + 1_000, 10);
        assert_eq!(summary.reserved_count, 1);
        assert_eq!(summary.reserved_total_micro_usd, 1_000);
        assert_eq!(summary.unresolved_count, 1);
        assert_eq!(summary.unresolved_total_micro_usd, 1_000);
        assert_eq!(summary.mismatch_count, 0);
        assert_eq!(summary.funded_total_micro_usd, 1_000_000);
        assert!(!summary.over_threshold);
        assert_eq!(summary.oldest_age_ms, Some(1_000));
        assert_eq!(summary.rows.len(), 2);
        // Oldest-first.
        assert_eq!(summary.rows[0].request_key, "chat:a");
        assert_eq!(summary.rows[1].request_key, "chat:b");
        assert_eq!(summary.rows[0].provider, PROVIDER);
        assert_eq!(summary.rows[0].model, MODEL);
    }

    #[test]
    fn get_reports_over_threshold_once_holds_pass_the_warn_share() {
        let db = test_db();
        // Funded for 2_000; reserving 1_500 total (75%) must trip the 50% threshold.
        let claims = fixture(&db, 1_000_000, 2_000);
        reserve(&db, &claims, "chat:big", NOW);
        db.reserve_provider_request(&claims, "chat:big2", "digest", 500, NOW)
            .unwrap();

        let summary = db.provider_holds_summary(NOW, 10);
        assert_eq!(summary.reserved_total_micro_usd, 1_500);
        assert!(summary.over_threshold);
    }

    // --- reserve_provider_request refuses a cancelled run's authorization ---

    #[test]
    fn reserve_provider_request_refuses_authorization_for_a_cancelled_run() {
        let db = test_db();
        let claims = fixture(&db, 1_000_000, 1_000_000);

        db.conn()
            .execute(
                "INSERT INTO runs (id, user_id, goal, status, created_at, updated_at)
                 VALUES ('run-1', 'tenant-1', 'goal', 'cancelled', ?1, ?1)",
                params![NOW],
            )
            .unwrap();

        let err = db
            .reserve_provider_request(&claims, "chat:cancelled-run", "digest", 1_000, NOW)
            .unwrap_err();
        assert_eq!(
            err, "spend authorization is missing, revoked, or expired",
            "an authorization whose run was cancelled must not be reservable, \
             even though the row itself is still `status = 'active'`"
        );
    }

    // --- settle / release happy paths ---

    #[test]
    fn settle_happy_path_moves_a_reserved_row_to_settled() {
        let db = test_db();
        let claims = fixture(&db, 1_000_000, 1_000_000);
        reserve(&db, &claims, "chat:settle", NOW);

        let settled = db
            .settle_provider_request("chat:settle", 800, None, NOW + 5)
            .unwrap();
        assert_eq!(settled.status, "settled");
        assert_eq!(settled.observed_micro_usd, Some(800));
    }

    #[test]
    fn release_happy_path_moves_a_reserved_row_to_released() {
        let db = test_db();
        let claims = fixture(&db, 1_000_000, 1_000_000);
        reserve(&db, &claims, "chat:release", NOW);

        let released = db
            .release_provider_request("chat:release", "operator released stuck hold", NOW + 5)
            .unwrap();
        assert_eq!(released.status, "released");
        assert_eq!(
            released.terminal_reason.as_deref(),
            Some("operator released stuck hold")
        );
    }

    #[test]
    fn release_also_resolves_an_unresolved_row() {
        let db = test_db();
        let claims = fixture(&db, 1_000_000, 1_000_000);
        reserve(&db, &claims, "chat:unresolved", NOW);
        db.mark_provider_request_unresolved("chat:unresolved", None, "timed out", NOW + 1)
            .unwrap();

        let released = db
            .release_provider_request("chat:unresolved", "confirmed never sent", NOW + 2)
            .unwrap();
        assert_eq!(released.status, "released");
    }

    // --- settle_provider_request already refuses to overwrite a terminal row ---

    #[test]
    fn settle_over_the_reserved_amount_flips_the_row_to_mismatch() {
        let db = test_db();
        let claims = fixture(&db, 1_000_000, 1_000_000);
        reserve(&db, &claims, "chat:mismatch", NOW);
        // Observed cost above the reservation flips the row to 'mismatch'
        // rather than settling it — the admin layer's `require_resolvable_hold`
        // (crates/api/src/admin.rs) checks this status before ever calling
        // settle/release again, so a 'mismatch' row can't be reached through
        // the admin endpoints once it lands here.
        let result = db.settle_provider_request("chat:mismatch", 5_000, None, NOW + 1);
        assert!(result.is_err());
        let row = db.get_provider_reservation("chat:mismatch").unwrap();
        assert_eq!(row.status, "mismatch");
    }

    #[test]
    fn release_on_a_settled_row_is_a_no_op_not_an_overwrite() {
        let db = test_db();
        let claims = fixture(&db, 1_000_000, 1_000_000);
        reserve(&db, &claims, "chat:settled", NOW);
        db.settle_provider_request("chat:settled", 500, None, NOW + 1)
            .unwrap();

        // `release_provider_request`'s WHERE clause only matches
        // reserved/unresolved, so this is a no-op that leaves the row
        // 'settled' rather than overwriting it.
        let after = db
            .release_provider_request("chat:settled", "attempted release", NOW + 2)
            .unwrap();
        assert_eq!(after.status, "settled");
    }
}

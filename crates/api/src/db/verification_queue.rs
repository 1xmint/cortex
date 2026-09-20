//! The durable verifier queue.
//!
//! Split out of `db.rs` for the same reason as `ledger.rs`: this is what makes
//! a delivery survive the process that accepted it, and it was buried between
//! unrelated impls.
//!
//! A move, not a redesign. The tests come with it -- they were already a named
//! module (`mod verifier`) and they are what proves the defences the
//! dispatcher documents: exclusive claims, a heartbeat from the wrong
//! dispatcher doing nothing, crash-after-claim reclaiming *and counting the
//! attempt*, exhausted attempts reaching dead rather than retrying.

use super::*;

impl Database {
    /// Enqueue a verification job **inside** an existing transaction.
    ///
    /// Private and transaction-taking on purpose. Design decision 4: the job is
    /// inserted in the same transaction that moves the step to `verifying`. If
    /// the transition commits the job exists; if it rolls back neither
    /// happened. That is the entire durability argument, and a public
    /// `enqueue` that could be called on its own would be a way around it.
    ///
    /// `UNIQUE(run_id, step_id, attempt_id, lease_gen)` makes a duplicate
    /// enqueue a no-op rather than a second receipt.
    /// `pub(super)` rather than private: the enqueue happens inside the
    /// delivery transaction in `mod.rs`, and the queue's own module is where
    /// the SQL belongs. Not `pub` -- nothing outside `db` may enqueue.
    pub(super) fn enqueue_verification_job_in(
        tx: &Connection,
        job_id: &str,
        run_id: &str,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        delivered_commit: &str,
        spec_set_digest: &str,
        runner_policy_ver: &str,
        now: i64,
    ) -> Result<(), String> {
        tx.execute(
            "INSERT INTO verification_jobs (
                job_id, run_id, step_id, attempt_id, lease_gen,
                delivered_commit, spec_set_id, quote_id, runner_policy_ver,
                state, attempt_count, next_run_at,
                created_at, updated_at, version
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, NULL, ?8,
                'queued', 0, ?9,
                ?9, ?9, 0
             )
             ON CONFLICT(run_id, step_id, attempt_id, lease_gen) DO NOTHING",
            params![
                job_id,
                run_id,
                step_id,
                attempt_id,
                lease_gen,
                delivered_commit,
                spec_set_digest,
                runner_policy_ver,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Claim the next runnable job.
    ///
    /// Expressed as a conditional update on `(state, claim_token)` — no
    /// `RETURNING`, no SQLite-specific construct — so the Postgres dispatcher's
    /// `FOR UPDATE SKIP LOCKED` can have the same semantics rather than merely
    /// a similar effect.
    ///
    /// The caller supplies the token, so a claim is attributable to a
    /// dispatcher rather than to whoever asked last.
    pub fn claim_verification_job(&self, claim_token: &str) -> Option<VerificationJob> {
        let mut conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let tx = conn.transaction().ok()?;

        let candidate: Option<String> = tx
            .query_row(
                "SELECT job_id FROM verification_jobs
                 WHERE state IN ('queued', 'retry_wait')
                   AND (next_run_at IS NULL OR next_run_at <= ?1)
                 ORDER BY next_run_at ASC, created_at ASC
                 LIMIT 1",
                params![now],
                |r| r.get(0),
            )
            .ok();
        let job_id = candidate?;

        // The CAS. Another dispatcher that read the same candidate loses here,
        // because the state it matched on has moved.
        let rows = tx
            .execute(
                "UPDATE verification_jobs
                 SET state = 'claimed', claim_token = ?1, claimed_at = ?2,
                     heartbeat_at = ?2, lease_expires_at = ?3,
                     attempt_count = attempt_count + 1,
                     updated_at = ?2, version = version + 1
                 WHERE job_id = ?4 AND state IN ('queued', 'retry_wait')",
                params![claim_token, now, now + VERIFICATION_LEASE_MS, job_id],
            )
            .unwrap_or(0);
        if rows == 0 {
            return None;
        }

        let job = read_verification_job(&tx, &job_id)?;
        tx.commit().ok()?;
        Some(job)
    }

    /// Extend a claim. Fails if the claim is no longer ours, which is how a
    /// dispatcher discovers it was reclaimed rather than carrying on and
    /// producing a verdict nobody will accept.
    pub fn heartbeat_verification_job(&self, job_id: &str, claim_token: &str) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE verification_jobs
             SET heartbeat_at = ?1, lease_expires_at = ?2, updated_at = ?1,
                 version = version + 1
             WHERE job_id = ?3 AND state = 'claimed' AND claim_token = ?4",
            params![now, now + VERIFICATION_LEASE_MS, job_id, claim_token],
        )
        .unwrap_or(0)
            > 0
    }

    /// Return claims whose lease expired to the queue, or retire them.
    ///
    /// Returns the number reclaimed. A job that has burned its attempts becomes
    /// `dead` instead of going round again: the failure mode this guards is a
    /// job that kills every dispatcher that touches it and takes the queue with
    /// it.
    pub fn reclaim_expired_verification_jobs(&self) -> usize {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();

        let dead = conn
            .execute(
                "UPDATE verification_jobs
                 SET state = 'dead', claim_token = NULL,
                     terminal_reason = 'claim expired after exhausting attempts',
                     updated_at = ?1, version = version + 1
                 WHERE state = 'claimed' AND lease_expires_at <= ?1
                   AND attempt_count >= ?2",
                params![now, VERIFICATION_MAX_ATTEMPTS],
            )
            .unwrap_or(0);

        let requeued = conn
            .execute(
                "UPDATE verification_jobs
                 SET state = 'retry_wait', claim_token = NULL, next_run_at = ?1,
                     updated_at = ?1, version = version + 1
                 WHERE state = 'claimed' AND lease_expires_at <= ?1
                   AND attempt_count < ?2",
                params![now, VERIFICATION_MAX_ATTEMPTS],
            )
            .unwrap_or(0);

        dead + requeued
    }

    /// Put a claimed job back for a bounded retry.
    ///
    /// This is what "no container runner available" becomes: a durable
    /// `retry_wait` with a time, rather than a log line and a stranded
    /// delivery.
    pub fn retry_verification_job(&self, job_id: &str, claim_token: &str, delay_ms: i64) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();

        // Out of attempts. Retrying forever is how a queue stops being a queue.
        let retired = conn
            .execute(
                "UPDATE verification_jobs
                 SET state = 'dead', claim_token = NULL,
                     terminal_reason = 'retries exhausted without a verdict',
                     updated_at = ?1, version = version + 1
                 WHERE job_id = ?2 AND state = 'claimed' AND claim_token = ?3
                   AND attempt_count >= ?4",
                params![now, job_id, claim_token, VERIFICATION_MAX_ATTEMPTS],
            )
            .unwrap_or(0);
        if retired > 0 {
            return true;
        }

        conn.execute(
            "UPDATE verification_jobs
             SET state = 'retry_wait', claim_token = NULL, next_run_at = ?1,
                 updated_at = ?2, version = version + 1
             WHERE job_id = ?3 AND state = 'claimed' AND claim_token = ?4",
            params![now + delay_ms, now, job_id, claim_token],
        )
        .unwrap_or(0)
            > 0
    }

    /// Seal a job's terminal state.
    ///
    /// `state` is `succeeded`, `failed`, `inconclusive`, or `dead`. Guarded on
    /// the claim, so a dispatcher whose claim expired mid-check cannot write a
    /// verdict for a job somebody else is now running.
    pub fn finish_verification_job(
        &self,
        job_id: &str,
        claim_token: &str,
        state: &str,
        reason: Option<&str>,
    ) -> bool {
        debug_assert!(
            matches!(state, "succeeded" | "failed" | "inconclusive" | "dead"),
            "a job finishes succeeded, failed, inconclusive, or dead"
        );
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE verification_jobs
             SET state = ?1, terminal_reason = ?2, claim_token = NULL,
                 lease_expires_at = NULL, updated_at = ?3, version = version + 1
             WHERE job_id = ?4 AND state = 'claimed' AND claim_token = ?5",
            params![state, reason, now, job_id, claim_token],
        )
        .unwrap_or(0)
            > 0
    }

    /// One job, whatever its state.
    pub fn get_verification_job(&self, job_id: &str) -> Option<VerificationJob> {
        let conn = self.conn();
        read_verification_job(&conn, job_id)
    }

    /// The job for a given attempt, if one was ever enqueued.
    pub fn get_verification_job_for_attempt(
        &self,
        run_id: &str,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
    ) -> Option<VerificationJob> {
        let conn = self.conn();
        let job_id: String = conn
            .query_row(
                "SELECT job_id FROM verification_jobs
                 WHERE run_id = ?1 AND step_id = ?2 AND attempt_id = ?3 AND lease_gen = ?4",
                params![run_id, step_id, attempt_id, lease_gen],
                |r| r.get(0),
            )
            .ok()?;
        read_verification_job(&conn, &job_id)
    }

    /// Every job that has not reached a terminal state.
    ///
    /// Deliberately not filtered by which process enqueued it — reconciliation
    /// exists to recover work stranded by the *previous* deploy, and a filter
    /// on this process would make it recover nothing that mattered.
    pub fn non_terminal_verification_jobs(&self) -> Vec<VerificationJob> {
        let conn = self.conn();
        let ids: Vec<String> = {
            let mut stmt = match conn.prepare(
                "SELECT job_id FROM verification_jobs
                 WHERE state IN ('queued', 'claimed', 'retry_wait')
                 ORDER BY created_at ASC",
            ) {
                Ok(stmt) => stmt,
                Err(_) => return Vec::new(),
            };
            let rows = match stmt.query_map([], |r| r.get::<_, String>(0)) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            };
            rows
        };
        ids.iter()
            .filter_map(|id| read_verification_job(&conn, id))
            .collect()
    }

    /// Record an operations-timeline event against a step.
    ///
    /// Public because the dispatcher needs it: a job that failed twice and
    /// succeeded on the third try is correct behaviour that nobody can see
    /// unless it is written down.
    pub fn record_step_operations_event(
        &self,
        step_id: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) {
        let conn = self.conn();
        insert_step_operations_event(&conn, step_id, event_type, payload);
    }

    /// Queue depth by state, for the metrics the operator watches.
    pub fn verification_queue_depth(&self) -> Vec<(String, i64)> {
        let conn = self.conn();
        let mut stmt = match conn
            .prepare("SELECT state, COUNT(*) FROM verification_jobs GROUP BY state ORDER BY state")
        {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)));
        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// The durable verifier queue: a delivery's verification survives the process
/// that accepted it.
///
/// Named for the crash each one recovers from. See
/// `cortex/plan/briefs/PR-B-durable-verifier.md`.
#[cfg(test)]
mod verifier {
    use super::tests::test_db;
    use super::*;

    fn spec(id: &str) -> cortex_core::verification::CheckSpec {
        cortex_core::verification::CheckSpec {
            id: id.to_string(),
            source: cortex_core::verification::CheckSource::Contract,
            command: vec!["true".to_string()],
            timeout_secs: 5,
            required: true,
        }
    }

    /// A run with one step leased, running, and delivered — the state a job is
    /// enqueued from.
    fn delivered_step(db: &Database, step_id: &str) -> (String, i64) {
        let now = Utc::now().timestamp_millis();
        let run_id = db.create_run_with_steps(
            "user-1",
            "ship it",
            "auto",
            &[],
            None,
            None,
            None,
            &[(
                step_id.to_string(),
                "execute".to_string(),
                "ship".to_string(),
                None,
                "execute".to_string(),
                "medium".to_string(),
                "Do the work".to_string(),
                now,
            )],
            &[],
        );
        db.update_run_status(&run_id, "running", None);
        db.register_worker("worker-1", "user-1");
        let lease_gen = db
            .lease_step(step_id, "worker-1", now + 600_000)
            .expect("step leases");
        assert!(db.start_step(step_id, lease_gen));
        assert!(db.deliver_step(step_id, "a1", lease_gen, None, None, None, Some("c0ffee")));
        (run_id, lease_gen)
    }

    fn enqueue<'a>(run_id: &'a str, job_id: &'a str, digest: &'a str) -> VerificationEnqueue<'a> {
        VerificationEnqueue {
            job_id,
            run_id,
            delivered_commit: "c0ffee",
            spec_set_digest: digest,
            runner_policy_ver: "runner-policy-1",
        }
    }

    #[test]
    fn job_insert_shares_transition_transaction() {
        // The whole durability argument. If the transition commits the job
        // exists; if it does not, neither does the job.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);

        // A transition that does not apply must leave no job behind. `lease_gen`
        // is the attempt discriminator — `attempt_id` travels with it as a
        // label, and the CAS is on the generation — so a stale generation is
        // what a superseded attempt actually looks like here.
        assert!(
            !db.begin_verifying_step(
                "step-1",
                "a1",
                gen - 1,
                Some(enqueue(&run_id, "job-x", &digest))
            ),
            "a transition for a superseded attempt must not apply"
        );
        assert!(
            db.get_verification_job("job-x").is_none(),
            "a rolled-back transition must not leave a job behind"
        );
        assert_eq!(
            db.get_step_status("step-1").as_deref(),
            Some("delivered"),
            "and must not have moved the step either"
        );

        // And the succeeding one brings its job with it.
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));
        let job = db.get_verification_job("job-1").expect("job exists");
        assert_eq!(job.state, "queued");
        assert_eq!(job.delivered_commit, "c0ffee");
        assert_eq!(job.spec_set_digest, digest);
        assert_eq!(db.get_step_status("step-1").as_deref(), Some("verifying"));
    }

    #[test]
    fn crash_after_enqueue_recovers() {
        // The process died before claiming. The job is still queued and
        // reconciliation can still see it, because it is a row and not a task.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));

        let pending = db.non_terminal_verification_jobs();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].job_id, "job-1");

        let claimed = db
            .claim_verification_job("dispatcher-2")
            .expect("claimable");
        assert_eq!(claimed.job_id, "job-1");
        assert_eq!(claimed.attempt_count, 1);
    }

    #[test]
    fn at_most_one_job_under_replay() {
        // A duplicate delivery for the same attempt must not produce a second
        // job, because a second job is a second receipt and a second charge.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);

        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));
        // The second transition is refused by the state machine anyway; the
        // unique key is the belt to that pair of braces.
        assert!(!db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-2", &digest))
        ));

        assert!(db.get_verification_job("job-2").is_none());
        let found = db
            .get_verification_job_for_attempt(&run_id, "step-1", "a1", gen)
            .expect("exactly one job");
        assert_eq!(found.job_id, "job-1");
    }

    #[test]
    fn a_claim_is_exclusive() {
        // Two dispatchers, one job. The loser gets nothing rather than a
        // duplicate — this is the CAS the Postgres port has to match.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));

        let first = db.claim_verification_job("dispatcher-a");
        let second = db.claim_verification_job("dispatcher-b");
        assert!(first.is_some());
        assert!(
            second.is_none(),
            "a claimed job must not be claimable again"
        );
        assert_eq!(
            db.get_verification_job("job-1")
                .unwrap()
                .claim_token
                .as_deref(),
            Some("dispatcher-a")
        );
    }

    #[test]
    fn a_heartbeat_from_the_wrong_dispatcher_does_nothing() {
        // How a dispatcher discovers it was reclaimed, instead of carrying on
        // and producing a verdict nobody will accept.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));
        db.claim_verification_job("dispatcher-a").expect("claim");

        assert!(db.heartbeat_verification_job("job-1", "dispatcher-a"));
        assert!(!db.heartbeat_verification_job("job-1", "dispatcher-b"));
    }

    #[test]
    fn crash_after_claim_reclaims_and_counts() {
        // A dispatcher died holding a claim. After the lease window another
        // takes it — and the attempt count moves, so a job that kills every
        // dispatcher it touches cannot loop forever.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));
        db.claim_verification_job("dispatcher-a").expect("claim");

        // Nothing to reclaim while the lease holds.
        assert_eq!(db.reclaim_expired_verification_jobs(), 0);

        expire_claim(&db, "job-1");
        assert_eq!(db.reclaim_expired_verification_jobs(), 1);

        let job = db.get_verification_job("job-1").unwrap();
        assert_eq!(job.state, "retry_wait");
        assert!(job.claim_token.is_none());
        assert_eq!(job.attempt_count, 1);

        let reclaimed = db
            .claim_verification_job("dispatcher-b")
            .expect("reclaimable");
        assert_eq!(
            reclaimed.attempt_count, 2,
            "reclaim must count as an attempt"
        );
    }

    #[test]
    fn exhausted_attempts_reach_dead_not_retry() {
        // A poison job stops rather than eating the queue.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));

        for _ in 0..VERIFICATION_MAX_ATTEMPTS {
            db.claim_verification_job("dispatcher-a")
                .expect("claimable");
            expire_claim(&db, "job-1");
            db.reclaim_expired_verification_jobs();
        }

        let job = db.get_verification_job("job-1").unwrap();
        assert_eq!(job.state, "dead", "a job that never completes must retire");
        assert!(job.terminal_reason.is_some(), "and say why");
        assert!(
            db.claim_verification_job("dispatcher-b").is_none(),
            "a dead job is not claimable"
        );
    }

    #[test]
    fn runner_unavailable_is_retry_wait_not_lost() {
        // The bug this PR exists to delete. "No container runner available"
        // used to be a log line and a permanently stranded delivery; it is now
        // a row with a time on it.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));
        db.claim_verification_job("dispatcher-a").expect("claim");

        assert!(db.retry_verification_job("job-1", "dispatcher-a", 60_000));

        let job = db.get_verification_job("job-1").unwrap();
        assert_eq!(job.state, "retry_wait");
        assert!(job.next_run_at.unwrap() > Utc::now().timestamp_millis());
        assert!(
            db.claim_verification_job("dispatcher-b").is_none(),
            "a job waiting to retry is not runnable yet"
        );
        assert_eq!(
            db.non_terminal_verification_jobs().len(),
            1,
            "and it is still visible to reconciliation"
        );
    }

    #[test]
    fn a_terminal_verdict_releases_the_claim() {
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));
        db.claim_verification_job("dispatcher-a").expect("claim");

        assert!(db.finish_verification_job("job-1", "dispatcher-a", "succeeded", None));
        let job = db.get_verification_job("job-1").unwrap();
        assert_eq!(job.state, "succeeded");
        assert!(job.claim_token.is_none());
        assert!(
            db.non_terminal_verification_jobs().is_empty(),
            "a finished job is not reconciliation's problem"
        );

        assert!(
            !db.finish_verification_job("job-1", "dispatcher-a", "failed", None),
            "a sealed job must not be resealed with a different verdict"
        );
        assert_eq!(db.get_verification_job("job-1").unwrap().state, "succeeded");
    }

    #[test]
    fn a_stale_dispatcher_cannot_seal_a_reclaimed_job() {
        // The dangerous version of the reclaim race: the original dispatcher
        // wakes up and writes a verdict for work somebody else now owns.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));
        db.claim_verification_job("dispatcher-a").expect("claim");
        expire_claim(&db, "job-1");
        db.reclaim_expired_verification_jobs();
        db.claim_verification_job("dispatcher-b").expect("reclaim");

        assert!(
            !db.finish_verification_job("job-1", "dispatcher-a", "succeeded", None),
            "the dispatcher that lost its claim must not seal the verdict"
        );
        assert_eq!(db.get_verification_job("job-1").unwrap().state, "claimed");
    }

    #[test]
    fn startup_reconciles_foreign_non_terminal_jobs() {
        // Reconciliation exists to recover what the *previous* deploy
        // stranded, so it must not be scoped to jobs this process enqueued.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));
        db.claim_verification_job("a-dispatcher-that-is-now-gone")
            .expect("claim");

        let pending = db.non_terminal_verification_jobs();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].state, "claimed");
        assert_eq!(
            pending[0].claim_token.as_deref(),
            Some("a-dispatcher-that-is-now-gone"),
            "a claim held by a process that no longer exists is exactly what \
             reconciliation is looking for"
        );
    }

    #[test]
    fn spec_set_is_frozen_at_enqueue() {
        // A job that re-resolved its specs at claim time could be graded
        // against a different exam than the one it was promised. The digest is
        // what makes that detectable rather than invisible.
        let one = spec_set_digest(&[spec("c1")]);
        let two = spec_set_digest(&[spec("c1"), spec("c2")]);
        assert_ne!(one, two, "a changed exam must produce a changed digest");
        assert_eq!(
            one,
            spec_set_digest(&[spec("c1")]),
            "and a stable one otherwise"
        );
        assert!(one.starts_with("sha256:"));
    }

    #[test]
    fn queue_depth_is_reportable() {
        // The operator-facing number. A queue nobody can see is a queue that
        // silently grows.
        let db = test_db();
        let (run_id, gen) = delivered_step(&db, "step-1");
        let digest = spec_set_digest(&[spec("c1")]);
        assert!(db.begin_verifying_step(
            "step-1",
            "a1",
            gen,
            Some(enqueue(&run_id, "job-1", &digest))
        ));

        let depth = db.verification_queue_depth();
        assert_eq!(depth, vec![("queued".to_string(), 1)]);
    }

    /// Force a claim's lease into the past, standing in for a dispatcher that
    /// died without releasing it.
    fn expire_claim(db: &Database, job_id: &str) {
        let conn = db.conn();
        conn.execute(
            "UPDATE verification_jobs SET lease_expires_at = 1 WHERE job_id = ?1",
            params![job_id],
        )
        .unwrap();
    }
}

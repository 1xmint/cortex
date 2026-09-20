/**
 * One-shot adoption script — plain JS, runs with node directly.
 * Works inside the production container (no tsx needed).
 *
 * Usage (host):      node scripts/adopt-legacy-keys.js
 * Usage (container): docker compose exec orchestrator node scripts/adopt-legacy-keys.js
 *
 * Reads env from .env file if dotenv is available, otherwise uses process.env.
 * Fully idempotent — safe to re-run.
 */

'use strict';

// Load .env if dotenv is available (host runs); container already has env injected.
try { require('dotenv').config(); } catch (_) {}

const path = require('path');
const { initDb, getDb } = require('./dist/db/index');
const { ClawNetApiKeyBackend } = require('./dist/core/api-key-rotation');

const CN_PATTERN = /^cn-[a-f0-9]{48}$/;

function main() {
  if (!process.env.CREDENTIAL_VAULT_KEK) {
    console.warn('⚠️  CREDENTIAL_VAULT_KEK not set — vault falls back to plain base64.');
  }

  initDb();
  const db = getDb();
  const backend = new ClawNetApiKeyBackend({ db });

  const rows = db.prepare('SELECT key, email, clerk_user_id FROM api_keys').all();
  console.log(`Found ${rows.length} total api_keys rows`);

  let adopted = 0, idempotent = 0, skipped = 0, errors = 0;

  for (const row of rows) {
    if (!CN_PATTERN.test(row.key)) {
      console.log(`  skip  ${row.key.slice(0, 10)}… (not cn- format)`);
      skipped++;
      continue;
    }

    const identityId = row.clerk_user_id || row.email;

    try {
      const result = backend.adoptPreVerifiedBearer({
        bearer: row.key,
        identityId,
        issuedAt: Date.now(),
        ttlMs: 10 * 60 * 1000,
      });

      if (result.idempotent) {
        console.log(`  idem  ${row.key.slice(0, 10)}… (already adopted)`);
        idempotent++;
      } else {
        console.log(`  adopt ${row.key.slice(0, 10)}… → identity ${identityId}`);
        adopted++;
      }
    } catch (err) {
      console.error(`  ERROR ${row.key.slice(0, 10)}…:`, err);
      errors++;
    }
  }

  console.log(`\nDone: ${adopted} adopted, ${idempotent} idempotent, ${skipped} skipped, ${errors} errors`);
}

main();

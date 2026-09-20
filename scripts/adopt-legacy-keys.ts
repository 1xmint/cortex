/**
 * One-shot adoption: register every legacy cn-[a-f0-9]{48} bearer from
 * the api_keys table into the rotation backend via adoptPreVerifiedBearer.
 *
 * Run: npx tsx --env-file=.env scripts/adopt-legacy-keys.ts
 *
 * Fully idempotent — re-running on a database where keys are already
 * adopted is a no-op per key (adoptPreVerifiedBearer handles re-runs).
 *
 * Skips keys that don't match the cn-[a-f0-9]{48} format (env keys,
 * delegated keys, or other formats).
 */

import 'dotenv/config';
import { initDb, getDb } from '../src/db/index';
import { ClawNetApiKeyBackend } from '../src/core/api-key-rotation';
import { env } from '../src/config/index';

const CN_PATTERN = /^cn-[a-f0-9]{48}$/;

interface ApiKeyRow {
  key: string;
  email: string;
  clerk_user_id: string | null;
}

function main() {
  // Warn if vault KEK is not set — adoption still works but secrets
  // are stored as plain base64 instead of AES-256-GCM.
  if (!env.CREDENTIAL_VAULT_KEK) {
    console.warn(
      '⚠️  CREDENTIAL_VAULT_KEK is not set — vault falls back to plain base64.',
      'Secrets will be functional but not encrypted at rest.',
    );
  }

  initDb();
  const db = getDb();
  const backend = new ClawNetApiKeyBackend({ db });

  const rows = db
    .prepare('SELECT key, email, clerk_user_id FROM api_keys')
    .all() as ApiKeyRow[];

  console.log(`Found ${rows.length} total api_keys rows`);

  let adopted = 0;
  let idempotent = 0;
  let skipped = 0;
  let errors = 0;

  for (const row of rows) {
    if (!CN_PATTERN.test(row.key)) {
      console.log(`  skip  ${row.key.slice(0, 10)}… (not cn- format)`);
      skipped++;
      continue;
    }

    // Use clerk_user_id as identity if available, otherwise fall back to email.
    const identityId = row.clerk_user_id || row.email;

    try {
      const result = backend.adoptPreVerifiedBearer({
        bearer: row.key,
        identityId,
        issuedAt: Date.now(),
        ttlMs: 10 * 60 * 1000, // 10 min — class A default
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

  console.log(
    `\nDone: ${adopted} adopted, ${idempotent} idempotent, ${skipped} skipped, ${errors} errors`,
  );
}

main();

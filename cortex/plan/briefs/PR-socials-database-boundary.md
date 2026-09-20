# PR brief — independent Socials database boundary

## Scope

- Add the internal `heyvera-db` crate without switching either running server.
- Replace the shared numeric migration counter with a checksummed Socials
  `0001_baseline` containing exactly 43 owned domain tables.
- Treat the Socials FTS index as derived state and rebuild it from posts.
- Inspect legacy sources through a read-only SQLite handle without emitting
  customer contents or opaque identifiers.
- Export only the explicit Socials table allowlist into a new database; never
  prune or mutate the mixed source in place.

## Acceptance evidence

- Fresh, repeated-open, checksum-tamper, integrity, foreign-key, FTS, DM, and
  subscription-history tests pass.
- A synthetic mixed v68 fixture retains Socials rows and excludes Cortex and
  mixed-table canaries.
- A current database initialized by the legacy v68 migrator exports cleanly
  into the independent baseline.
- The schema validator rejects every one of the 61 Cortex domain-table names.

## Operational boundary

This branch creates no production database, backup, credential, deployment,
or repository. Runtime remains on the mixed database until the separate
application/state graph in PR 4 is ready and explicitly cut over later.

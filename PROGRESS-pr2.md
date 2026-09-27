1. Corrected seed_models() Anthropic rates + added missing rows (Opus 5.5/4.8/4.7/4.5, Sonnet 4.5, Fable 5/5.1); OpenAI rows left, marked UNVERIFIED re: reasoning tokens.
2. Pinned rate test pinned_anthropic_rates_match_the_published_price_list_2026_09_27 added, citing platform.claude.com pricing page + 2026-09-27.
3. margin_bp set to 0 in seed_provisional, whole-credit round-up removed (floor); comment explains ClassPrice.margin_bp kept only for legacy db round-trip, never bills (Provisional).
4. Added pure UsageTokens/cost_micro_usd/charge/micro_usd_to_dollars_display in pricing.rs with unit tests (incl. property test).
5. Migration v72 added (db/mod.rs): credit_transactions.cost_micro_usd, credit_balances.carry_micro_usd; SCHEMA_VERSION bumped to 72; SCHEMA_FINGERPRINT NOT yet updated (needs CI's reported value). New tests/billing_schema.rs.
6. Not implemented: provider_gateway.rs is forbidden for this PR; punch-list item only (see report).

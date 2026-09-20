# Provider gateway capability delivery

## Objective

Carry one short-lived, finite Claude gateway capability from the brain to the
sandbox without persisting or logging the bearer token, and expose a stub-only
Anthropic Messages listener that exercises the already-merged reservation path.

## Bounded change

- Add an optional, redacted gateway-access envelope to `ExecuteStep`.
- Preserve fail-closed behavior across old/new protocol version skew.
- Configure Claude Code with its documented `ANTHROPIC_BASE_URL` and
  `ANTHROPIC_AUTH_TOKEN` variables only when the envelope matches the job.
- Route Claude sandbox egress to Cortex's gateway host, not directly to the
  supplier.
- Add a capability-authenticated `/internal/provider/v1/messages` listener in
  explicit stub mode only.
- Prove direct supplier access is absent and the stub listener reserves and
  reconciles exactly once.

## Exclusions

No supplier transport, live provider key, live call, purchase, deployment,
streaming, routing, second provider, automatic retry, comparison run, billing
migration, npm pilot, memory redesign, or repository split.

## Acceptance

1. The bearer token is redacted from debug output and absent from execution-job
   receipts.
2. Missing, expired, cross-attempt, cross-model, non-HTTPS, or non-gateway-host
   access produces no gateway environment variables.
3. A matching envelope produces only the documented Claude gateway variables
   plus the existing constant scratch environment.
4. Claude egress names Cortex's gateway and cannot reach `api.anthropic.com`.
5. The listener is unavailable unless stub mode is explicit, requires a valid
   capability and request idempotency key, and uses the durable gateway engine.
6. All upstream behavior remains stubbed and provider spend remains `$0`.

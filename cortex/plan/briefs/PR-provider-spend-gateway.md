# Private provider spending gateway

## Objective

Establish the boundary a live-model experiment must cross: the supplier
credential remains outside the sandbox, each request is authorized for one
tenant/run/attempt/model and a finite amount, and durable supplier spend is
reserved before any upstream transport is invoked.

## Bounded change

- Add a signed, short-lived gateway capability restricted to tenant, run,
  attempt, provider, model, spending authorization, and expiry.
- Add durable supplier spending authorizations and request reservations.
- Reserve conservatively from immutable model-rate data before forwarding.
- Reconcile observed usage exactly once; release unused reservation capacity.
- Keep timeouts and successful responses with missing usage unresolved until a
  later reconciliation record supplies the answer.
- Reject replay conflicts, cross-tenant identifiers, expired/tampered
  capabilities, unsupported providers/models, and unbounded request forms
  before invoking the transport.
- Remove supplier API keys from the sandbox environment. A provider CLI must
  ultimately receive only a gateway capability.
- Exercise the gateway with an in-process stub transport only.

## Exclusions

No live provider call, purchase, deployment, provider routing, second provider,
automatic retry, customer billing-policy migration, npm environment work,
memory redesign, or repository split. Wiring capability issuance through the
worker protocol is a subsequent bounded change after this storage and gateway
boundary passes.

## Acceptance

1. The gateway invokes its transport only after an atomic durable reservation.
2. Concurrent/replayed requests cannot exceed an authorization or global
   supplier capacity and do not create duplicate reservations or spend rows.
3. A successful response with observed usage settles at actual cost and frees
   the unused portion of the reservation.
4. A timeout or missing usage remains unresolved and continues to consume its
   full reservation until explicit reconciliation.
5. Capability tampering, expiry, model mismatch, tenant mismatch, and an
   unbounded request are rejected before the transport sees them.
6. A reconciliation replay is a no-op; contradictory reconciliation is a
   visible mismatch, never an overwrite.
7. No supplier credential is present in a sandbox environment or gateway
   response.

## Verification

```text
cargo test -p cortex-api --all-targets --locked provider_gateway
cargo test -p cortex-worker --all-targets --locked sandbox
cargo test --workspace --all-targets --locked
cargo test --workspace --all-targets --no-default-features --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Provider spending remains `$0`; all upstream behavior is stubbed.

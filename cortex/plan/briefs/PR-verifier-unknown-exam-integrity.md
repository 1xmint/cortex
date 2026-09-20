# Fail closed on unknown exam integrity

## Objective

The verifier must not grade a delivery unless it can establish whether the
frozen exam remained intact. Replace the current permissive
`Option<Vec<String>>` guard with explicit outcomes for an intact exam,
permitted authored work, a modified exam, and unknown integrity with a
diagnostic reason.

## Bounded change

- Missing or unreadable work contracts, missing required base commits, and
  failed diff inspection produce `Verdict::Inconclusive`.
- Persist the diagnostic in the existing verification lifecycle.
- Unknown integrity runs no grading checks, causes no ledger movement, and
  cannot create a positive routing signal.
- Preserve strong-contract grading, authored-work behavior, and the
  no-frozen-checks/no-delivery path.
- Add driver tests for verdicts, runner call counts, diagnostics, ledger
  effects, and replay.

## Exclusions

No billing-policy migration, provider or gateway work, model execution,
memory redesign, npm environment work, repository split, deployment, purchase,
or pull request creation.

## Acceptance

1. Intact strong work grades normally.
2. A modified protected exam is inconclusive.
3. Missing contract, unreadable contract, missing base, and failed diff
   inspection are inconclusive with distinct diagnostics.
4. Unknown integrity runs zero checks and changes no credit balance.
5. Replay creates no duplicate verification or ledger effect.
6. Existing PASS, FAIL, and NOOP classifications remain unchanged.

## Verification

```text
cargo test -p cortex-api --all-targets --locked verification_driver
cargo test -p cortex-api --all-targets --locked --test step_end_to_end
cargo test --workspace --all-targets --locked
cargo test --workspace --all-targets --no-default-features --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The existing stub-provider procedure remains the integration check. Live-model
execution and provider spending remain disabled.

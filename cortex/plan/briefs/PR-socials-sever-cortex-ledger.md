# PR brief — sever Socials from the Cortex ledger

## Goal

Make the first Cortex extraction boundary behaviorally true inside HeyVera:
Socials must neither read nor mutate Cortex credit, usage, quote, receipt, or
provider-spend state.

## In scope

- Return HTTP 503 with code `SOCIALS_BILLING_UNAVAILABLE` from new Pulse draft
  creation and new Socials checkout.
- Return the same stable code from Pulse chat's `create_draft` tool without
  creating a draft.
- Keep existing draft reads, approval, rejection, publication, audit,
  schedules, and goals working.
- Keep existing Socials subscription status, billing history, portal access,
  and Stripe lifecycle maintenance working.
- Give the HeyVera router a Socials-specific billing handler surface.
- Remove all Pulse calls to Cortex credit and usage methods.
- Hide Socials credit/automation totals and new checkout in the live frontend.

## Deliberately out of scope

- Choosing the eventual Socials price, entitlement, or metering model.
- Changing Cortex ledger, quote, receipt, refund, reservation, or provider-spend
  behavior.
- Route-manifest, database-crate, repository-history, deployment, credential,
  DNS, or production changes.

## Verification

- HTTP integration test: draft creation, checkout, and usage return the stable
  503 response and leave seeded Cortex balance/usage rows unchanged.
- Unit tests: the chat tool creates no draft; Socials Stripe events preserve
  subscription lifecycle/history while leaving credit rows absent or unchanged.
- Run the complete Pulse test filter and all HeyVera unit/type/build checks.
- Run the workspace all-target suite, no-default-features suite, clippy with
  warnings denied, and the repository formatting gate before handoff.

## Rollback

Before deployment, revert this change. After deployment, retain the 503 gate at
the router or proxy and never restore Socials ledger mutation while unwinding
other parts of the release.

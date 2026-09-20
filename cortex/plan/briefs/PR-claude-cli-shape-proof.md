# Claude CLI gateway request-shape proof

## Objective

Prove the real, version-pinned Claude CLI honors Cortex's gateway environment
contract and record the bounded request shape the gateway must support, without
network access, a supplier credential, a provider call, or provider spend.

## Bounded change

- Refresh the pinned Claude CLI used by the provider sandbox image.
- Run that image with `--network none`, a read-only root filesystem, and a
  loopback-only capture server.
- Supply the worker's gateway environment contract with a loopback base URL,
  bearer, attempt header, routed model, and production print-mode flags.
- Assert the emitted Messages path, authorization scheme, attempt identity,
  model, output bound, streaming mode, and tool-field state.
- Keep the existing adversarial proof that a Claude grant cannot contact the
  supplier directly.

## Exclusions

No gateway relaxation, supplier transport, live key, live call, purchase,
deployment, hosted test service, second provider, billing change, or repository
split.

## Acceptance

1. The proof fails if the real CLI no longer sends the scoped bearer or attempt
   header to the configured gateway.
2. The proof fails if the routed model or positive `max_tokens` bound is absent.
3. The proof records that this real print-mode invocation requires streaming
   and currently sends an empty `tools` array; either shape changing fails CI for
   review.
4. The proof container has no network interface beyond loopback, so passing it
   cannot spend money or reach a supplier.
5. The gateway remains fail closed for unsupported shapes; compatibility is a
   later, separately reviewed change informed by this evidence.

# Claude CLI through the Rust stub listener

## Objective

Close the gap between the captured Claude CLI request and the Rust gateway by
running the pinned CLI against the actual stub listener in an isolated
container.

## Bounded change

- Build a feature-gated, loopback-only proof binary around the production stub
  handler.
- Run the pinned Claude CLI against `/internal/provider/v1/messages` with no
  container network.
- Require the CLI to consume the synthetic SSE response and exit successfully.
- Admit only structurally bounded tool-definition arrays; tool execution stays
  entirely inside the existing sandbox boundary.
- Query the same proof database and require every finite request to settle.

## Exclusions

No supplier transport, external network, live key, live request, purchase,
deployment, tool-result simulation, production proof endpoint, or repository
split.

## Acceptance

1. The proof binary is absent unless `gateway-cli-proof` is explicitly enabled
   and refuses to start without its proof-mode guard.
2. It binds only `127.0.0.1` and the container runs with `--network none`, a
   read-only root, no capabilities, and no privilege escalation.
3. The pinned CLI successfully consumes the real Rust listener's SSE response.
4. The durable database reports one to four reservations and all are settled;
   more calls are a pinned-CLI contract change that fails the proof.
5. Existing request-shape assertions and all required checks remain green.

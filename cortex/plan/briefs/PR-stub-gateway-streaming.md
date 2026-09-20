# Stub gateway streaming compatibility

## Objective

Make the private stub listener compatible with the exact Claude CLI request
shape proven in PR #644 while preserving settlement-before-response and keeping
all supplier and non-empty tool behavior disabled.

## Bounded change

- Accept boolean streaming and the measured empty `tools` array.
- Continue rejecting non-boolean streaming values and every non-empty or
  non-array tools value before a reservation is created.
- Convert the already-settled synthetic message into a valid Anthropic SSE
  event sequence for the CLI.
- Prove the streamed response settles one durable spend row exactly once.

## Exclusions

No supplier streaming transport, live key, live call, purchase, deployment,
non-empty tool definitions, tool-result turns, second provider, retry policy,
or repository split.

## Acceptance

1. The measured `stream: true, tools: []` request passes bounded validation.
2. The response is `text/event-stream` with a complete Anthropic event
   sequence the pinned CLI can consume.
3. Reservation settlement finishes before the buffered stub stream is
   returned, and replay cannot create a second spend row.
4. Non-empty tools and malformed stream values fail before transport.
5. Stub mode remains the only HTTP transport implementation and provider spend
   remains `$0`.

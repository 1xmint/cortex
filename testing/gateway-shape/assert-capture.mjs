import { readFileSync } from "node:fs";

const capturePath = process.env.CORTEX_CAPTURE_PATH ?? "/scratch/claude-request.json";
const capture = JSON.parse(readFileSync(capturePath, "utf8"));
const failures = [];
const check = (condition, message) => {
  if (!condition) failures.push(message);
};

check(capture.method === "POST", `expected POST, received ${capture.method}`);
check(
  typeof capture.url === "string" && capture.url.startsWith("/v1/messages"),
  `expected /v1/messages request, received ${capture.url}`,
);
check(
  capture.headers.authorization === "Bearer cortex-shape-token",
  "ANTHROPIC_AUTH_TOKEN did not become the scoped Bearer authorization",
);
check(
  capture.headers["x-cortex-attempt"] === "attempt-shape-proof",
  "ANTHROPIC_CUSTOM_HEADERS did not preserve the attempt identity",
);
check(
  capture.body.model === "claude-sonnet-4-6",
  `routed model was not preserved: ${capture.body.model}`,
);
check(
  Number.isInteger(capture.body.max_tokens) && capture.body.max_tokens > 0,
  "request did not carry a positive integer max_tokens bound",
);
check(Array.isArray(capture.body.messages) && capture.body.messages.length > 0, "messages absent");
check(capture.body.stream === true, "Claude CLI did not request streaming");
check(
  Array.isArray(capture.body.tools) && capture.body.tools.length === 0,
  "production print-mode tools must remain an empty array; review non-empty tools before accepting them",
);
check(capture.headers["x-api-key"] === undefined, "CLI also exposed an x-api-key credential");

if (failures.length > 0) {
  for (const failure of failures) process.stderr.write(`shape failure: ${failure}\n`);
  process.exit(1);
}

process.stdout.write(
  JSON.stringify({
    path: capture.url,
    auth: "Bearer [REDACTED]",
    attempt: capture.headers["x-cortex-attempt"],
    model: capture.body.model,
    max_tokens: capture.body.max_tokens,
    stream: capture.body.stream,
    tool_count: capture.body.tools.length,
  }) + "\n",
);

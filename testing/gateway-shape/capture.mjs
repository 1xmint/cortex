import { writeFileSync } from "node:fs";
import { createServer } from "node:http";

const capturePath = process.env.CORTEX_CAPTURE_PATH ?? "/scratch/claude-request.json";
const port = Number.parseInt(process.env.CORTEX_CAPTURE_PORT ?? "18080", 10);
const maxBodyBytes = 2 * 1024 * 1024;
let captured = false;

const server = createServer((request, response) => {
  if (request.method === "GET" && request.url === "/health") {
    response.writeHead(200, { "content-type": "text/plain" });
    response.end("ready");
    return;
  }

  const chunks = [];
  let bytes = 0;
  request.on("data", (chunk) => {
    bytes += chunk.length;
    if (bytes > maxBodyBytes) {
      request.destroy(new Error("request body exceeded capture limit"));
      return;
    }
    chunks.push(chunk);
  });
  request.on("end", () => {
    let body;
    try {
      body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    } catch {
      response.writeHead(400, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: { message: "invalid JSON" } }));
      return;
    }

    if (!captured && request.method === "POST" && request.url?.startsWith("/v1/messages")) {
      captured = true;
      writeFileSync(
        capturePath,
        JSON.stringify(
          {
            method: request.method,
            url: request.url,
            headers: request.headers,
            body,
          },
          null,
          2,
        ),
      );
    }

    if (body.stream === true) {
      response.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
      });
      const events = [
        [
          "message_start",
          {
            type: "message_start",
            message: {
              id: "msg_cortex_shape",
              type: "message",
              role: "assistant",
              model: body.model,
              content: [],
              stop_reason: null,
              stop_sequence: null,
              usage: { input_tokens: 1, output_tokens: 0 },
            },
          },
        ],
        [
          "content_block_start",
          {
            type: "content_block_start",
            index: 0,
            content_block: { type: "text", text: "" },
          },
        ],
        [
          "content_block_delta",
          {
            type: "content_block_delta",
            index: 0,
            delta: { type: "text_delta", text: "cortex gateway shape stub" },
          },
        ],
        ["content_block_stop", { type: "content_block_stop", index: 0 }],
        [
          "message_delta",
          {
            type: "message_delta",
            delta: { stop_reason: "end_turn", stop_sequence: null },
            usage: { output_tokens: 6 },
          },
        ],
        ["message_stop", { type: "message_stop" }],
      ];
      for (const [event, data] of events) {
        response.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
      }
      response.end();
    } else {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(
        JSON.stringify({
          id: "msg_cortex_shape",
          type: "message",
          role: "assistant",
          model: body.model,
          content: [{ type: "text", text: "cortex gateway shape stub" }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 1, output_tokens: 6 },
        }),
      );
    }
  });
});

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`capture server ready on ${port}\n`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}

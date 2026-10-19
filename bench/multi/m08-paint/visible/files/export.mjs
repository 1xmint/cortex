import fs from "node:fs";
import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, exportPng } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await stroke(page, [[50, 50], [350, 250]]);
  const download = await exportPng(page);
  check("the file is drawing.png", download.suggestedFilename() === "drawing.png", download.suggestedFilename());
  const bytes = fs.readFileSync(await download.path());
  check("it is a PNG", bytes.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])));
  check("it is 400 by 300", bytes.length > 24 && bytes.readUInt32BE(16) === 400 && bytes.readUInt32BE(20) === 300, bytes.length > 24 ? `${bytes.readUInt32BE(16)}x${bytes.readUInt32BE(20)}` : "too short");
});

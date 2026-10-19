import fs from "node:fs";
import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, colour, exportPng } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await colour(page, "#ff0000");
  await stroke(page, [[50, 150], [350, 150]]);
  const download = await exportPng(page);
  const data = fs.readFileSync(await download.path()).toString("base64");
  const got = await page.evaluate(async (data) => {
    const img = new Image();
    img.src = `data:image/png;base64,${data}`;
    await img.decode();
    const c = document.createElement("canvas");
    c.width = img.naturalWidth;
    c.height = img.naturalHeight;
    const g = c.getContext("2d");
    g.drawImage(img, 0, 0);
    return { size: `${c.width}x${c.height}`, red: [...g.getImageData(200, 150, 1, 1).data], far: [...g.getImageData(200, 40, 1, 1).data] };
  }, data);
  check("the file is 400 by 300", got.size === "400x300", got.size);
  check("the file shows the red stroke", got.red[0] > 215 && got.red[1] < 40 && got.red[2] < 40 && got.red[3] === 255, String(got.red));
  check("the background is solid white", got.far.join() === "255,255,255,255", String(got.far));
});

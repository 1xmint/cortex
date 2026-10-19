import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, clear, pixel } from "./common.mjs";

const white = (px) => px.join() === "255,255,255,255";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("the canvas starts solid white", white(await pixel(page, 10, 10)) && white(await pixel(page, 390, 290)), String(await pixel(page, 10, 10)));
  await stroke(page, [[10, 10], [390, 290]]);
  await clear(page, true);
  check("clear makes it solid white again", white(await pixel(page, 200, 150)), String(await pixel(page, 200, 150)));
});

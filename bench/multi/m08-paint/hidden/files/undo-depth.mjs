import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, dot, click, pixel, near, blank, allBlank } from "./common.mjs";

const black = async (page, x, y) => near(await pixel(page, x, y), [0, 0, 0]);

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await click(page, "#undo");
  await click(page, "#redo");
  check("undo and redo on an empty canvas do nothing", await allBlank(page));

  const ys = [50, 150, 250];
  for (const y of ys) await stroke(page, [[50, y], [150, y]]);
  for (let i = 0; i < 3; i++) await click(page, "#undo");
  check("three undos take back three strokes", await allBlank(page));
  await click(page, "#undo");
  check("one more undo does nothing", await allBlank(page));
  for (let i = 0; i < 3; i++) await click(page, "#redo");
  for (const y of ys) check(`redo puts back the stroke at ${y}`, await black(page, 100, y));
  await click(page, "#redo");
  check("one more redo changes nothing", (await black(page, 100, 250)) && blank(await pixel(page, 300, 150)));

  await click(page, "#undo");
  await stroke(page, [[250, 150], [350, 150]]);
  await click(page, "#redo");
  check("a new stroke after undo leaves nothing to redo", blank(await pixel(page, 100, 250)), String(await pixel(page, 100, 250)));
  check("the new stroke stays", await black(page, 300, 150));

  // Twenty more dots along the bottom, then twenty undos: the strokes from before stay.
  for (let i = 0; i < 20; i++) await dot(page, 15 + i * 18, 290);
  check("twenty dots drawn", (await black(page, 15, 290)) && (await black(page, 15 + 19 * 18, 290)));
  for (let i = 0; i < 20; i++) await click(page, "#undo");
  check("twenty undos take back twenty dots", blank(await pixel(page, 15, 290)) && blank(await pixel(page, 15 + 19 * 18, 290)));
  check("and nothing earlier", (await black(page, 300, 150)) && (await black(page, 100, 50)));
});

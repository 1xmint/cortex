import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, dot, pixel, near, blank, allBlank, active } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("the canvas is 400 by 300", (await page.locator("#canvas").evaluate((c) => [c.width, c.height].join("x"))) === "400x300");
  check("the canvas starts empty", await allBlank(page));
  check("the pen is chosen", (await active(page, "#pen")) && !(await active(page, "#eraser")));
  check("black is chosen", await active(page, '.colour[data-colour="#000000"]'));
  check("size 8 is chosen", await active(page, '.size[data-size="8"]'));

  await stroke(page, [[50, 50], [150, 50]]);
  check("a stroke draws in black", near(await pixel(page, 100, 50), [0, 0, 0]), String(await pixel(page, 100, 50)));
  check("only where the mouse went", blank(await pixel(page, 100, 80)));

  await stroke(page, [[250, 50], [300, 100], [350, 50]]);
  check("a stroke follows a bent path", near(await pixel(page, 300, 100), [0, 0, 0]), String(await pixel(page, 300, 100)));

  await dot(page, 200, 150);
  check("a press without moving draws a dot", near(await pixel(page, 200, 150), [0, 0, 0]), String(await pixel(page, 200, 150)));
  check("a small one", blank(await pixel(page, 200, 140)));
});

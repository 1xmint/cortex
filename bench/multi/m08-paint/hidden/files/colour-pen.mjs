import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, click, colour, size, pixel, near, active } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await size(page, 20);
  await click(page, "#eraser");
  await colour(page, "#0000ff");
  check("choosing a colour chooses the pen", (await active(page, "#pen")) && !(await active(page, "#eraser")));
  await stroke(page, [[50, 100], [350, 100]]);
  check("and it draws in that colour", near(await pixel(page, 200, 100), [0, 0, 255]), String(await pixel(page, 200, 100)));
  check("at the chosen size", near(await pixel(page, 200, 107), [0, 0, 255]));

  await click(page, "#eraser");
  await size(page, 2);
  await stroke(page, [[200, 50], [200, 150]]);
  check("the eraser uses the chosen size", near(await pixel(page, 200, 100), [255, 255, 255]) && near(await pixel(page, 205, 100), [0, 0, 255]));
  check("only one colour is chosen", (await page.locator(".colour.active").count()) === 1);
});

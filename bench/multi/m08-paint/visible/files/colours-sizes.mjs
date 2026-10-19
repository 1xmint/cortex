import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, colour, size, pixel, near, active, thickness } from "./common.mjs";

const colours = [
  ["#000000", [0, 0, 0]],
  ["#ff0000", [255, 0, 0]],
  ["#00ff00", [0, 255, 0]],
  ["#0000ff", [0, 0, 255]],
  ["#ffff00", [255, 255, 0]],
];

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("five colours in order", (await page.locator(".colour").evaluateAll((b) => b.map((x) => x.dataset.colour).join())) === colours.map((c) => c[0]).join());
  for (const [i, [hex, rgb]] of colours.entries()) {
    const y = 30 + i * 40;
    await colour(page, hex);
    await stroke(page, [[20, y], [120, y]]);
    check(`${hex} draws in that colour`, near(await pixel(page, 70, y), rgb), String(await pixel(page, 70, y)));
    check(`${hex} shows as chosen`, await active(page, `.colour[data-colour="${hex}"]`));
  }

  await colour(page, "#000000");
  for (const [n, y, low, high] of [[2, 40, 1, 4], [8, 120, 6, 10], [20, 220, 17, 23]]) {
    await size(page, n);
    await stroke(page, [[200, y], [380, y]]);
    const got = await thickness(page, 290, y - 30, y + 30);
    check(`size ${n} draws ${n} pixels wide`, got >= low && got <= high, String(got));
    check(`size ${n} shows as chosen`, await active(page, `.size[data-size="${n}"]`));
  }
});

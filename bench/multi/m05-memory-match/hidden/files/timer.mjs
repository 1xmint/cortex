import { withPage, check } from "./lib/cortex-check.mjs";
import { open, text, flip, size, win, visible } from "./common.mjs";

await withPage(async (page, ctx) => {
  await page.clock.install();
  await open(page, ctx);
  // Hold the clock still so a slow machine cannot add real time between steps.
  await page.clock.pauseAt(Date.now() + 1000);
  check("time starts at 0", (await text(page, "#time")) === "0");
  await page.clock.runFor(5000);
  check("time waits for the first flip", (await text(page, "#time")) === "0", await text(page, "#time"));

  await flip(page, 0);
  await page.clock.runFor(3500);
  check("time counts whole seconds", (await text(page, "#time")) === "3", await text(page, "#time"));
  await page.clock.runFor(2000);
  check("time keeps counting", (await text(page, "#time")) === "5", await text(page, "#time"));

  await size(page, "easy", 8);
  await flip(page, 0);
  await page.clock.runFor(2500);
  await win(page);
  check("the game is won", await visible(page, "#won"));
  const final = await text(page, "#time");
  check("the win time is shown", final === "2", final);
  await page.clock.runFor(5000);
  check("time stops on a win", (await text(page, "#time")) === final, await text(page, "#time"));
});

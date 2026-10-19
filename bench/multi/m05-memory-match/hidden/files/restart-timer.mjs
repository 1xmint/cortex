import { withPage, check } from "./lib/cortex-check.mjs";
import { open, text, flip, hasClass } from "./common.mjs";

const restart = (page) => page.locator("#restart").click({ timeout: 2000 });

await withPage(async (page, ctx) => {
  await page.clock.install();
  await open(page, ctx);
  // Hold the clock still so a slow machine cannot add real time between steps.
  await page.clock.pauseAt(Date.now() + 1000);
  await flip(page, 0);
  await page.clock.runFor(4000);
  check("time is running", (await text(page, "#time")) === "4", await text(page, "#time"));

  await restart(page);
  check("restart sets time to 0", (await text(page, "#time")) === "0", await text(page, "#time"));
  await page.clock.runFor(3000);
  check("time waits for the next first flip", (await text(page, "#time")) === "0", await text(page, "#time"));
  await flip(page, 0);
  await page.clock.runFor(2000);
  check("time counts the new game", (await text(page, "#time")) === "2", await text(page, "#time"));

  // A miss still showing when restart is pressed must not reach the new game.
  await flip(page, 2);
  await restart(page);
  await flip(page, 0);
  await page.clock.runFor(1500);
  check("a card from the new game stays up", await hasClass(page, 0, "flipped"));
  await flip(page, 1);
  check("the new game takes clicks", await hasClass(page, 1, "matched"));
});

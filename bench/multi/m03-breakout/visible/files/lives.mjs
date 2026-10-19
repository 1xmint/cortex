import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, setBall, text } from "./common.mjs";

const drop = async (page) => {
  await setBall(page, 100, 330, 0, 0.3);
  await step(page, 5);
};

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);

  await drop(page);
  let s = await state(page);
  check("a fallen ball costs a life", s.lives === 2 && s.screen === "playing", s);
  check("lives are shown", (await text(page, "#lives")) === "2");
  check("lost screen stays hidden", !(await page.locator("#lost").isVisible()));

  await drop(page);
  await drop(page);
  s = await state(page);
  check("the third fall ends the game", s.lives === 0 && s.screen === "lost", s);
  check("lost screen shows", await page.locator("#lost").isVisible());
});

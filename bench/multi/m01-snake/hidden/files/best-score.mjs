import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, food, text, TICK } from "./common.mjs";

async function playTo(page, points) {
  await startGame(page);
  for (let i = 0; i < points; i++) {
    await food(page, 11 + i, 10);
    await step(page, TICK);
    await food(page, 0, 19);
  }
  await step(page, TICK * 20);
}

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await playTo(page, 2);
  let s = await state(page);
  check("game over with score 2", s.over && s.score === 2, JSON.stringify(s));
  check("best shows 2", (await text(page, "#best")) === "2");
  await open(page, ctx);
  s = await state(page);
  check("best is still 2 after a reload", s.best === 2, JSON.stringify(s));
  check("best element shows 2 after a reload", (await text(page, "#best")) === "2");
  await playTo(page, 1);
  s = await state(page);
  check("a lower score keeps best at 2", s.best === 2 && (await text(page, "#best")) === "2", JSON.stringify(s));
});

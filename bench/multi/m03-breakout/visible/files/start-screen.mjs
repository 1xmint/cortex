import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, text } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("start screen shows on load", await page.locator("#start-screen").isVisible());
  check("state says start", (await state(page)).screen === "start");

  await startGame(page);
  const s = await state(page);
  check("start screen hides", !(await page.locator("#start-screen").isVisible()));
  check("game is playing level 1", s.screen === "playing" && s.level === 1, s);
  check("score 0 and 3 lives", s.score === 0 && s.lives === 3, s);
  check("level 1 has 32 bricks", s.bricks.length === 32, s.bricks.length);
  const xs = s.bricks.map((b) => `${b.x},${b.y},${b.w},${b.h}`);
  check("bricks sit where the prompt says", xs.includes("4,40,56,16") && xs.includes("424,100,56,16"), xs);
  const p = s.paddle;
  check("paddle starts at x 200", p.x === 200 && p.y === 300 && p.w === 80 && p.h === 10, p);
  const b = s.ball;
  check("ball starts at 240,280", b.x === 240 && b.y === 280 && b.vx === 0.15 && b.vy === -0.3 && b.r === 5, b);
  const shown = [await text(page, "#score"), await text(page, "#lives"), await text(page, "#level")];
  check("score, lives and level are shown", shown.join(" ") === "0 3 1", shown);
});

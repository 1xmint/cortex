// Helpers shared by this task's checks. Every check opens the page in test
// mode, so the game moves only when a check calls step().
export async function open(page, ctx, query = "?test=1") {
  page.setDefaultTimeout(5000);
  await page.goto(ctx.url("index.html" + query));
}

export const state = (page) => page.evaluate(() => window.game.state());
export const step = (page, ms) => page.evaluate((ms) => window.game.step(ms), ms);
export const setBall = (page, x, y, vx, vy) =>
  page.evaluate(([x, y, vx, vy]) => window.game.setBall(x, y, vx, vy), [x, y, vx, vy]);
export const text = async (page, sel) => (await page.textContent(sel))?.trim();

export async function startGame(page) {
  await page.locator("#start").click({ timeout: 2000 });
}

/** Breaks the lowest standing brick by sending the ball up into it. */
export const breakLowest = (page) =>
  page.evaluate(() => {
    const s = window.game.state();
    const br = [...s.bricks].sort((a, b) => b.y - a.y)[0];
    window.game.setBall(br.x + br.w / 2, br.y + br.h + 6, 0, -0.3);
    window.game.step(10);
  });

/** Breaks bricks until the level changes or the game stops; at most 60. */
export const clearLevel = (page) =>
  page.evaluate(() => {
    const g = window.game;
    const level = g.state().level;
    for (let i = 0; i < 60; i++) {
      const s = g.state();
      if (s.screen !== "playing" || s.level !== level || s.bricks.length === 0) return;
      const br = [...s.bricks].sort((a, b) => b.y - a.y)[0];
      g.setBall(br.x + br.w / 2, br.y + br.h + 6, 0, -0.3);
      g.step(10);
    }
  });

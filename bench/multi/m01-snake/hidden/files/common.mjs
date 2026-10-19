// Helpers shared by this task's checks. Every check opens the page in test
// mode, so the snake moves only when a check calls step().
export const TICK = 150;

export async function open(page, ctx, query = "?test=1") {
  page.setDefaultTimeout(5000);
  await page.goto(ctx.url("index.html" + query));
}

export const state = (page) => page.evaluate(() => window.game.state());
export const step = (page, ms) => page.evaluate((ms) => window.game.step(ms), ms);
export const food = (page, x, y) => page.evaluate(([x, y]) => window.game.placeFood(x, y), [x, y]);
export const head = (s) => `${s.snake[0].x},${s.snake[0].y}`;
export const cells = (s) => s.snake.map((c) => `${c.x},${c.y}`).join(" ");
export const text = async (page, sel) => (await page.textContent(sel))?.trim();

/** Starts a game and parks the food in a corner no check drives through. */
export async function startGame(page) {
  await page.locator("#start").click({ timeout: 2000 });
  await food(page, 0, 19);
}

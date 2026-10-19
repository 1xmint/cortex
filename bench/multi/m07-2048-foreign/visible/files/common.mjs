// Helpers shared by this task's checks.

/** Lets slides, spawns and redraws finish. */
export const settle = (page) => page.waitForTimeout(300);

/** Opens the game (with an optional query such as "?seed=7") and waits for its tiles. */
export async function open(page, ctx, query = "") {
  page.setDefaultTimeout(5000);
  await page.goto(ctx.url("index.html") + query);
  await page.locator(".tile-container .tile").first().waitFor({ state: "attached" });
  await settle(page);
}

/** The board as rows of numbers (0 for empty), read from the tile classes. */
export const grid = (page) =>
  page.locator(".tile-container .tile").evaluateAll((tiles) => {
    const rows = [0, 1, 2, 3].map(() => [0, 0, 0, 0]);
    for (const tile of tiles) {
      const value = /(?:^| )tile-(\d+)(?: |$)/.exec(tile.className);
      const at = /tile-position-(\d)-(\d)/.exec(tile.className);
      if (!value || !at) continue;
      const [x, y] = [Number(at[1]) - 1, Number(at[2]) - 1];
      // Merging tiles briefly share a cell with their result; keep the result.
      rows[y][x] = Math.max(rows[y][x], Number(value[1]));
    }
    return rows;
  });

/** The board as one string, for comparing. */
export const board = async (page) => (await grid(page)).map((r) => r.join(",")).join("/");

export async function press(page, key) {
  await page.keyboard.press(key);
  await settle(page);
}

const number = async (page, sel) => parseInt((await page.textContent(sel)) ?? "", 10);
export const score = (page) => number(page, ".score-container");
export const best = (page) => number(page, ".best-container");
export const moves = async (page) => (await page.textContent("#moves"))?.trim();

export async function undo(page) {
  await page.locator("#undo").click({ timeout: 2000 });
  await settle(page);
}

export async function restart(page) {
  await page.locator(".restart-button").click({ timeout: 2000 });
  await settle(page);
}

/**
 * Loads a saved game in the original format: `rows` top to bottom, 0 for
 * empty. `extra` adds or overrides fields of the save.
 */
export async function load(page, ctx, rows, query = "", extra = {}) {
  await open(page, ctx, query);
  await page.evaluate(
    ({ rows, extra }) => {
      const cells = [0, 1, 2, 3].map((x) =>
        [0, 1, 2, 3].map((y) => (rows[y][x] ? { position: { x, y }, value: rows[y][x] } : null)),
      );
      const state = { grid: { size: 4, cells }, score: 0, over: false, won: false, keepPlaying: false, ...extra };
      localStorage.setItem("gameState", JSON.stringify(state));
    },
    { rows, extra },
  );
  await page.reload();
  await page.locator(".tile-container .tile").first().waitFor({ state: "attached" });
  await settle(page);
}

/** The loaded board's string form. */
export const shape = (rows) => rows.map((r) => r.join(",")).join("/");

/** True when every tile and tile face has no transition and no animation. */
export const still = (page) =>
  page.locator(".tile-container .tile, .tile-container .tile-inner").evaluateAll((els) =>
    els.length > 0 &&
    els.every((el) => {
      const s = getComputedStyle(el);
      const zero = (list) => list.split(",").every((t) => parseFloat(t) === 0);
      return zero(s.transitionDuration) && (s.animationName === "none" || zero(s.animationDuration));
    }),
  );

/** True when the tiles slide with a transition. */
export const slides = (page) =>
  page.locator(".tile-container .tile").first().evaluate((el) =>
    getComputedStyle(el).transitionDuration.split(",").some((t) => parseFloat(t) > 0),
  );

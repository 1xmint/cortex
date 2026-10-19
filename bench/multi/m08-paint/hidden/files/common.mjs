// Helpers shared by this task's checks.

export async function open(page, ctx) {
  page.setDefaultTimeout(5000);
  await page.goto(ctx.url("index.html"));
  await page.locator("#canvas").waitFor({ state: "visible" });
}

/** The canvas's top-left corner on the page. */
const corner = async (page) => {
  const box = await page.locator("#canvas").boundingBox();
  return { x: box.x, y: box.y };
};

/** Draws a stroke through `points` (canvas coordinates). */
export async function stroke(page, points) {
  const at = await corner(page);
  const [first, ...rest] = points;
  await page.mouse.move(at.x + first[0], at.y + first[1]);
  await page.mouse.down();
  for (const [x, y] of rest) await page.mouse.move(at.x + x, at.y + y, { steps: 8 });
  await page.mouse.up();
}

/** Presses and lets go without moving. */
export const dot = (page, x, y) => stroke(page, [[x, y]]);

export const click = (page, sel) => page.locator(sel).click({ timeout: 2000 });
export const colour = (page, hex) => click(page, `.colour[data-colour="${hex}"]`);
export const size = (page, n) => click(page, `.size[data-size="${n}"]`);

/** The pixel at x,y as [r, g, b, a]. */
export const pixel = (page, x, y) =>
  page.locator("#canvas").evaluate((c, [x, y]) => [...c.getContext("2d").getImageData(x, y, 1, 1).data], [x, y]);

/** True when `px` is within a small distance of the colour `rgb`, fully opaque. */
export const near = (px, rgb) => px[3] > 230 && rgb.every((v, i) => Math.abs(px[i] - v) <= 40);

/** Blank means white, or nothing drawn at all. */
export const blank = (px) => px[3] === 0 || near(px, [255, 255, 255]);

/** True when no pixel on the canvas has been drawn on. */
export const allBlank = (page) =>
  page.locator("#canvas").evaluate((c) => {
    const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data;
    for (let i = 0; i < d.length; i += 4) {
      const white = d[i] > 215 && d[i + 1] > 215 && d[i + 2] > 215;
      if (d[i + 3] !== 0 && !white) return false;
    }
    return true;
  });

/** How many pixels of column x, from y0 to y1, are not blank: the stroke's thickness there. */
export const thickness = (page, x, y0, y1) =>
  page.locator("#canvas").evaluate(
    (c, [x, y0, y1]) => {
      const d = c.getContext("2d").getImageData(x, y0, 1, y1 - y0).data;
      let n = 0;
      for (let i = 0; i < d.length; i += 4) {
        const white = d[i] > 127 && d[i + 1] > 127 && d[i + 2] > 127;
        if (d[i + 3] > 127 && !white) n += 1;
      }
      return n;
    },
    [x, y0, y1],
  );

export const active = (page, sel) => page.locator(sel).evaluate((el) => el.classList.contains("active"));

/** Clicks Clear and answers its question with `agree`. Returns whether a question was asked. */
export async function clear(page, agree) {
  let asked = false;
  const answer = async (dialog) => {
    asked = dialog.type() === "confirm";
    await (agree ? dialog.accept() : dialog.dismiss());
  };
  page.once("dialog", answer);
  await click(page, "#clear");
  await page.waitForTimeout(200);
  page.off("dialog", answer);
  return asked;
}

/** Clicks Export and returns the download. */
export async function exportPng(page) {
  const [download] = await Promise.all([page.waitForEvent("download", { timeout: 5000 }), click(page, "#export")]);
  return download;
}

// Helpers shared by this task's checks.
export async function open(page, ctx) {
  page.setDefaultTimeout(5000);
  await page.goto(ctx.url("index.html"));
}

export async function add(page, text) {
  await page.locator("#new-card").fill(text);
  await page.locator("#add").click({ timeout: 2000 });
}

/** The texts of the cards shown in a column, top to bottom. */
export const texts = (page, col) =>
  page.evaluate(
    (col) =>
      [...document.querySelectorAll(`#col-${col} .card`)]
        .filter((c) => c.checkVisibility())
        .map((c) => c.querySelector(".text")?.textContent.trim()),
    col,
  );

export const count = async (page, col) => (await page.locator(`#col-${col} .count`).textContent())?.trim();

/** The card whose text is exactly `text`. */
export const card = (page, text) =>
  page.locator(".card").filter({ has: page.locator(".text", { hasText: new RegExp(`^\s*${text}\s*$`) }) });

export async function press(page, text, button) {
  await card(page, text).locator(`.${button}`).click({ timeout: 2000 });
}

export async function edit(page, text, value, key) {
  await press(page, text, "edit");
  const input = page.locator(".edit-input");
  await input.fill(value);
  await input.press(key);
}

export const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

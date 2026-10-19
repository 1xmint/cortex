// Helpers shared by this task's checks. In test mode cards 2k and 2k+1
// (counting from 0) are a pair.

/** Opens the game in test mode and waits for the cards. */
export async function open(page, ctx) {
  page.setDefaultTimeout(5000);
  await page.goto(ctx.url("index.html?test=1"));
  await page.locator("#board .card").first().waitFor({ state: "visible" });
}

export const text = async (page, sel) => (await page.textContent(sel))?.trim();
export const cards = (page) => page.locator("#board .card");
export const count = (page) => cards(page).count();
export const flip = (page, i) => cards(page).nth(i).click({ timeout: 2000 });
export const visible = (page, sel) => page.locator(sel).isVisible();
export const hasClass = async (page, i, name) =>
  ((await cards(page).nth(i).getAttribute("class")) ?? "").split(/\s+/).includes(name);
export const faceUp = (page) => page.locator("#board .card.flipped").count();

/** Chooses a size and waits for the new deal. Polls from here, so it works while a check holds the page's clock still. */
export async function size(page, value, cardsExpected) {
  await page.selectOption("#size", value);
  for (let i = 0; i < 50 && (await cards(page).count()) !== cardsExpected; i++) await new Promise((r) => setTimeout(r, 100));
  const got = await cards(page).count();
  if (got !== cardsExpected) throw new Error(`expected ${cardsExpected} cards after choosing ${value}, saw ${got}`);
}

/** How many cards sit on the first row, by where they are drawn. */
export const columns = (page) =>
  page.evaluate(() => {
    const all = [...document.querySelectorAll("#board .card")];
    const top = all[0].getBoundingClientRect().top;
    return all.filter((c) => Math.abs(c.getBoundingClientRect().top - top) < 2).length;
  });

/** Turns every pair over in order, winning in exactly cards/2 moves. */
export async function win(page) {
  const n = await count(page);
  for (let i = 0; i < n; i += 2) {
    await flip(page, i);
    await flip(page, i + 1);
  }
}

/** Turns over two cards that do not match, then waits for them to turn back. */
export async function miss(page) {
  await flip(page, 0);
  await flip(page, 2);
  await page.waitForFunction(() => document.querySelectorAll("#board .card.flipped").length === 0);
}

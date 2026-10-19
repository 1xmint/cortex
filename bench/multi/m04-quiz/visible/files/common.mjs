// Helpers shared by this task's checks.

/** Opens the quiz and waits for the start button, which shows once loaded. */
export async function open(page, ctx) {
  page.setDefaultTimeout(5000);
  await page.goto(ctx.url("index.html"));
  await page.locator("#start").waitFor({ state: "visible" });
}

/** The questions the page is serving, read the way the page reads them. */
export const served = (page) => page.evaluate(() => fetch("questions.json").then((r) => r.json()));
export const text = async (page, sel) => (await page.textContent(sel))?.trim();
export const choices = async (page) => (await page.locator(".choice").allTextContents()).map((t) => t.trim());
export const pick = (page, i) => page.locator(".choice").nth(i).click({ timeout: 2000 });
export const visible = (page, sel) => page.locator(sel).isVisible();

/** Answers every question still to come, choosing `choose(question)`. */
export async function finish(page, questions, choose) {
  const asked = [];
  for (let n = 0; n < questions.length && !(await visible(page, "#results")); n++) {
    const shown = await text(page, "#question");
    const q = questions.find((q) => q.question === shown);
    if (!q) throw new Error(`unknown question shown: ${shown}`);
    asked.push(shown);
    await pick(page, choose(q));
  }
  return asked;
}

// Helpers shared by this task's checks.

/** Opens the app and waits for the form. */
export async function open(page, ctx) {
  page.setDefaultTimeout(5000);
  await page.goto(ctx.url("index.html"));
  await page.locator("#add").waitFor({ state: "visible" });
}

export const text = async (page, sel) => (await page.textContent(sel))?.trim();
export const visible = (page, sel) => page.locator(sel).isVisible();

/** Fills the form and submits it. Fields left out are left as they are. */
export async function submit(page, { description, amount, date, category } = {}) {
  if (description !== undefined) await page.fill("#description", description);
  if (amount !== undefined) await page.fill("#amount", String(amount));
  if (date !== undefined) await page.fill("#date", date);
  if (category !== undefined) await page.selectOption("#category", category);
  await page.locator("#save").click({ timeout: 2000 });
}

/** The list as shown, top to bottom. */
export const rows = (page) =>
  page.locator("#list .expense").evaluateAll((items) =>
    items.map((li) => {
      const part = (cls) => li.querySelector(`.${cls}`)?.textContent.trim();
      return {
        date: part("date"),
        description: part("description"),
        category: part("category"),
        amount: part("amount"),
      };
    }),
  );

/** The row whose description is `description`. */
export const row = (page, description) =>
  page.locator("#list .expense").filter({
    has: page.locator(".description", { hasText: new RegExp(`^${description}$`) }),
  });

export async function setMonth(page, value) {
  await page.fill("#month", value);
  await page.locator("#month").dispatchEvent("change");
}

/** The five category totals the chart carries. */
export const chartTotals = (page) =>
  page.locator("#chart").evaluate((c) =>
    Object.fromEntries(["food", "transport", "home", "fun", "other"].map((k) => [k, c.getAttribute(`data-total-${k}`)])),
  );

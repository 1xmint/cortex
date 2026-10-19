import { withPage, check } from "./lib/cortex-check.mjs";
import { open, text, choices, visible, finish } from "./common.mjs";

// A different file from the one the task ships with: the quiz must ask these.
const custom = [
  { question: "Which is a prime number?", choices: ["4", "6", "7", "9"], answer: 2 },
  { question: "Which gas do plants take in?", choices: ["Carbon dioxide", "Helium", "Neon", "Argon"], answer: 0 },
  { question: "How many sides has a hexagon?", choices: ["Five", "Six", "Seven", "Eight"], answer: 1 },
];

await withPage(async (page, ctx) => {
  await page.route("**/questions.json", (route) =>
    route.fulfill({ contentType: "application/json", body: JSON.stringify(custom) }),
  );
  await open(page, ctx);
  await page.locator("#start").click({ timeout: 2000 });
  check("first question comes from the file", (await text(page, "#question")) === custom[0].question);
  const shown = await choices(page);
  check("its choices come from the file", JSON.stringify(shown) === JSON.stringify(custom[0].choices), shown);

  const asked = await finish(page, custom, (q) => q.answer);
  check("every question in the file is asked", JSON.stringify(asked) === JSON.stringify(custom.map((q) => q.question)), asked);
  check("results show after the last one", await visible(page, "#results"));
  check("final score counts the file's questions", (await text(page, "#final-score")) === "3 / 3");
  check("review has one line per question", (await page.locator("#review li").count()) === 3);
});

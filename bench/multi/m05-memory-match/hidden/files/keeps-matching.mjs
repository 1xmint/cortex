import { withPage, check } from "./lib/cortex-check.mjs";
import { open, flip, hasClass, size, faceUp } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await flip(page, 0);
  await flip(page, 1);
  check("a pair is marked matched", (await hasClass(page, 0, "matched")) && (await hasClass(page, 1, "matched")));
  check("a matched pair is face up", (await hasClass(page, 0, "flipped")) && (await hasClass(page, 1, "flipped")));

  await flip(page, 2);
  await flip(page, 4);
  check("a miss is face up at first", (await hasClass(page, 2, "flipped")) && (await hasClass(page, 4, "flipped")));
  await flip(page, 6);
  check("clicks wait while a miss is showing", !(await hasClass(page, 6, "flipped")));
  await page.waitForTimeout(1500);
  check("a miss turns back over", !(await hasClass(page, 2, "flipped")) && !(await hasClass(page, 4, "flipped")));
  check("the matched pair stays up", (await hasClass(page, 0, "matched")) && (await faceUp(page)) === 2);

  await size(page, "hard", 36);
  await flip(page, 34);
  await flip(page, 35);
  check("test mode pairs the last two hard cards", await hasClass(page, 35, "matched"));
  await flip(page, 0);
  await flip(page, 35 - 2);
  await page.waitForTimeout(1500);
  check("a miss turns back over on hard too", !(await hasClass(page, 0, "flipped")));
});

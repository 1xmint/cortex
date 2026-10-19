import { withPage, check } from "./lib/cortex-check.mjs";
import { open, text, flip } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("moves start at 0", (await text(page, "#moves")) === "0");

  await flip(page, 0);
  check("one card is not a move", (await text(page, "#moves")) === "0");
  await flip(page, 1);
  check("a matching pair is one move", (await text(page, "#moves")) === "1");

  await flip(page, 2);
  await flip(page, 4);
  check("a miss is a move too", (await text(page, "#moves")) === "2");
});

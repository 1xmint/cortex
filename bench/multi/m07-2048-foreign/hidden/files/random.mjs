import { withPage, check } from "./lib/cortex-check.mjs";
import { open, board, restart } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const seen = new Set([await board(page)]);
  for (let i = 0; i < 4; i++) {
    await restart(page);
    seen.add(await board(page));
  }
  check("without a seed, new games differ", seen.size >= 2, [...seen].join(" | "));
});

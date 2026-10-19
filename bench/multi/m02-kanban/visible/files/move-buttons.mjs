import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, count, press } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await add(page, "Task");
  await press(page, "Task", "right");
  check("right moves To do to Doing", (await texts(page, "doing"))[0] === "Task");
  check("counts follow", (await count(page, "todo")) === "0" && (await count(page, "doing")) === "1");
  await press(page, "Task", "right");
  check("right moves Doing to Done", (await texts(page, "done"))[0] === "Task");
  await press(page, "Task", "right");
  check("right does nothing in Done", (await texts(page, "done"))[0] === "Task");
  await press(page, "Task", "left");
  check("left moves Done to Doing", (await texts(page, "doing"))[0] === "Task");
  check("Done count back to 0", (await count(page, "done")) === "0");
});

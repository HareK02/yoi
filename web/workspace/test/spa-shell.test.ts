import { assertEquals } from "jsr:@std/assert";
import { ssr } from "../src/routes/+layout.ts";

Deno.test("workspace frontend serves deep links through the client SPA", () => {
  assertEquals(ssr, false);
});

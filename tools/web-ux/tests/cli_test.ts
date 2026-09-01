import { assertEquals, assertThrows } from "@std/assert";
import { parseArguments } from "../cli.ts";
import { isVisibleUiErrorText } from "../src/capture.ts";

Deno.test("CLI parses bounded capture filters", () => {
  const parsed = parseArguments([
    "capture",
    "--scenario",
    "scenario.json",
    "--personas",
    "owner,non-owner",
    "--headed",
  ]);
  assertEquals(parsed.command, "capture");
  assertEquals(parsed.values.get("personas"), ["owner,non-owner"]);
  assertEquals(parsed.flags.has("headed"), true);
});

Deno.test("visible UI error classification ignores ordinary status text", () => {
  assertEquals(isVisibleUiErrorText("Refresh failed (401 Unauthorized)"), true);
  assertEquals(isVisibleUiErrorText("Workspace list loaded"), false);
});

Deno.test("CLI rejects positional and missing option values", () => {
  assertThrows(() => parseArguments(["capture", "scenario.json"]), Error, "unexpected argument");
});

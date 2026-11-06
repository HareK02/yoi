import { verifyStartupHttpCaptures } from "./worker-startup-backend-contract.ts";
import { startupFixture } from "../src/lib/workspace/workers/startup-failure.test-fixtures.ts";

// bridge自身の試験。合成markerを実handler実行の証拠とは扱わない。
function capture(name: "denial" | "lost") {
  const body = startupFixture(name);
  return `T812_BROWSER_STARTUP_PROBLEM status=${body.status} ignored reason phrase content_type=application/problem+json body=${
    JSON.stringify(body)
  }`;
}
async function rejects(log: string) {
  try {
    await verifyStartupHttpCaptures(log);
  } catch (error) {
    if (String(error).includes("test-private-body")) {
      throw new Error("Capture body disclosed");
    }
    return;
  }
  throw new Error(
    "Expected the capture bridge to reject incomplete/invalid evidence",
  );
}

Deno.test("Startup HTTP bridge requires both captured cases and passes real response shape through the production Web caller", async () => {
  const result = await verifyStartupHttpCaptures(
    `unrelated test output\n${capture("denial")}\n${capture("lost")}\n`,
  );
  if (result.records !== 2 || result.cases !== 2) {
    throw new Error("Unexpected capture coverage");
  }
});
Deno.test("Startup HTTP bridge refuses missing, malformed, oversized, mismatched and legacy captures without exposing raw body", async () => {
  const denial = capture("denial");
  const lost = capture("lost");
  for (
    const log of [
      "",
      denial,
      `${denial}\n${denial}`,
      `${denial}\n${lost.replace("status=502", "status=500")}`,
      `${denial}\n${
        lost.replace(
          "content_type=application/problem+json",
          "content_type=application/json",
        )
      }`,
      `${denial}\nT812_BROWSER_STARTUP_PROBLEM status=502 content_type=application/problem+json body=test-private-body`,
      `${denial}\nT812_BROWSER_STARTUP_PROBLEM status=502 content_type=application/problem+json body=${
        "x".repeat(32769)
      }`,
      `${denial}\nT812_BROWSER_STARTUP_PROBLEM test-private-body`,
    ]
  ) await rejects(log);
});

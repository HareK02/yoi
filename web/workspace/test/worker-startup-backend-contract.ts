// 実認可・Runtime・保存を通したRust connected試験の公開HTTP捕捉だけを受ける。
// Web送信/受信/表示modelへの接続であり、このprocess自体は認可や保存を代替証明しない。
import { buildCreateWorkspaceWorkerRequest } from "../src/lib/workspace/sidebar/worker-launch.ts";
import {
  submitWorkerLaunch,
  workerLaunchNeedsReconciliation,
  workerStartupNotice,
} from "../src/lib/workspace/workers/startup-failure.ts";

const marker = "T812_BROWSER_STARTUP_PROBLEM ";
const maxLogBytes = 16 * 1024 * 1024;

export async function verifyStartupHttpCaptures(log: string) {
  if (new TextEncoder().encode(log).length > maxLogBytes) {
    throw new Error("Startup capture log exceeds the test input limit");
  }
  const request = buildCreateWorkspaceWorkerRequest({
    runtime_id: "embedded",
    display_name: "HTTP contract check",
    profile: "",
    subjektiv_subject_id: "",
    initial_text: "",
    workdir_attachments: [],
    working_directory_repository_key: "",
    working_directory_selector: "HEAD",
  });
  const cases = new Set<string>();
  let records = 0;
  for (const line of log.split("\n")) {
    if (!line.includes(marker)) continue;
    const metadata = line.slice(line.indexOf(marker) + marker.length);
    const match =
      /^status=(\d{3})(?: [A-Za-z ]+)? content_type=([^ ]+) body=(.*)$/u.exec(
        metadata,
      );
    if (!match) throw new Error("Malformed Startup HTTP capture metadata");
    const [, status, contentType, body] = match;
    let calls = 0;
    const path = "/api/w/contract/workers";
    const result = await submitWorkerLaunch(
      ((url: unknown, init?: { method?: string; body?: unknown }) => {
        calls++;
        if (
          url !== path || init?.method !== "POST" ||
          init.body !== JSON.stringify(request)
        ) {
          throw new Error("Startup caller did not preserve the launch request");
        }
        return Promise.resolve(
          new Response(body, {
            status: Number(status),
            headers: { "content-type": contentType },
          }),
        );
      }) as typeof fetch,
      path,
      request,
    );
    if (
      calls !== 1 || result.kind !== "failed" ||
      result.receipt.kind !== "problem"
    ) {
      // 受信bodyや他のRust log行を例外へ転載しない。
      throw new Error(
        "Actual Startup HTTP capture was not accepted by the Web contract",
      );
    }
    const receipt = result.receipt;
    const failure = receipt.problem.yoi_failure;
    const cause = failure.details;
    if (
      failure.domain !== "session_registration" ||
      !("required_operation" in cause) ||
      cause.required_operation !== "session.record"
    ) {
      throw new Error("Expected connected Session registration HTTP captures");
    }
    if (
      cause.reason === "permission_denied" && cause.principal === "subject" &&
      Number(status) === 409
    ) cases.add("denial");
    else if (cause.reason === "outcome_unknown" && Number(status) === 502) {
      cases.add("response_lost");
    } else throw new Error("Unexpected connected Session registration result");
    if (
      !receipt.extension ||
      receipt.extension.operation.execution !== "errored" ||
      receipt.extension.backend_result_persistence !== "persisted"
    ) {
      throw new Error(
        "Connected owner execution and saved result observations were not retained",
      );
    }
    if (
      workerStartupNotice(receipt).explanation !== receipt.problem.detail ||
      !workerLaunchNeedsReconciliation(receipt)
    ) {
      throw new Error(
        "Web cause presentation or uncertain-result gating differs from the connected HTTP result",
      );
    }
    records++;
  }
  if (!cases.has("denial") || !cases.has("response_lost")) {
    throw new Error(
      "Missing real denial or response-loss Startup HTTP capture",
    );
  }
  return { records, cases: cases.size };
}

if (import.meta.main) {
  const path = Deno.args[0];
  if (!path) {
    throw new Error("Expected the connected browser_http test output path");
  }
  if ((await Deno.stat(path)).size > maxLogBytes) {
    throw new Error("Startup capture log exceeds the test input limit");
  }
  const result = await verifyStartupHttpCaptures(await Deno.readTextFile(path));
  console.log(
    `Actual Backend Startup responses accepted by production Web caller, decoder, notice and gating: ${result.records} records, ${result.cases} cases`,
  );
}

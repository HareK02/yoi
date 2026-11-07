import { ComposerAdmissions } from "../../web/workspace/src/lib/workspace/console/composer-delivery.ts";
import type {
  Event,
  Method,
} from "../../web/workspace/src/lib/generated/protocol.ts";

// Rustの実WS結合試験で得たACKを既存Consoleの受付管理へ渡す。
// モデル・ブラウザ描画は対象外。Denoは通常テストrunnerと同じ前提。
type Exchange = {
  method: Extract<Method, { method: "notify" }>;
  event: Event;
  accepted: boolean;
  retry: boolean;
};
const exchanges: Exchange[] = JSON.parse(Deno.args[0]);
const admissions = new ComposerAdmissions();
for (const exchange of exchanges) {
  const { method, event, accepted, retry } = exchange;
  const snapshot = {
    document: method.params.message,
    content: method.params.message,
    segments: [{ kind: "text" as const, content: method.params.message }],
    pastes: [],
    textPastes: [],
  };
  const record = admissions.begin("real-worker", method, snapshot, "notify");
  if (JSON.stringify(record.method) !== JSON.stringify(method)) {
    throw new Error("次のdraftが前の受付に置き換わった");
  }
  if (retry) {
    admissions.disconnected("real-worker");
    if (admissions.get("real-worker")?.status !== "unknown") {
      throw new Error("ACK消失が拒否または成功になった");
    }
    if (
      JSON.stringify(admissions.retry("real-worker")?.method) !==
        JSON.stringify(method)
    ) {
      throw new Error("再送で入力またはIDが変わった");
    }
  }
  const result = admissions.acknowledge("real-worker", event);
  if (!result || result.accepted !== accepted) {
    throw new Error(
      `実WSのACKが元のIDへ相関しない: ${JSON.stringify(exchange)}`,
    );
  }
  if (
    accepted
      ? admissions.get("real-worker") !== null
      : admissions.get("real-worker")?.status !== "rejected"
  ) {
    throw new Error("ACK後の受付状態が不正");
  }
}
console.log(
  `Console accepted/rejected/retry/next-draft: ${exchanges.length} real WS exchanges`,
);

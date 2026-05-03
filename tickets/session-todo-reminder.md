# セッション内 Task ツールの注意機構

## 背景

`tickets/session-todo.md` で導入する Task ツール群があっても、LLM はそれを使わずに作業を続け得る。ツールを呼ばないまま会話が長引くと、

- 開始した作業の `inprogress` がずっと放置されたままになる
- 「やったつもり」になって `completed` への更新を忘れる
- そもそも TaskStore の存在を忘れて、構造化を諦めて自由記述に回帰する

OpenCode の todo は専用の注意機構を持たない（汎用 reminder 経由）。一方 Claude Code は `task_reminder` を「N リクエスト無アクティビティで初めて発火するナッジ型」として実装しており、毎リクエスト押し戻しはしない（`<local-agent-install>` の local reminder helper functions、閾値 `a local reminder threshold`）。

Insomnia でも同方針を採り、active Task が残っているのに `TaskCreate` / `TaskUpdate` が一定リクエスト呼ばれていない場合に限り、`<system-reminder>` で揮発的に思い出させる。「やったつもり」抑止と、トークン浪費・LLM の自律性侵害のバランスを取るため、毎リクエスト押し戻しはしない。

## 前提

- `tickets/session-todo.md` の TaskStore と `TaskCreate` / `TaskUpdate` / `TaskList` / `TaskGet` ツールが利用可能であること
- `pre_llm_request` 相当のフックを Pod が持つこと（無ければ本ticket で導入）

## 方針

- **`pre_llm_request` Interceptor として実装**。直近の user message に `<system-reminder>` ブロックを揮発的に append するだけ。履歴・ログには載せない
- **system-reminder 注入の汎用化はやらない**。利用者が Task 1機構しかない段階で抽象を立てない（CLAUDE.md「概念の追加は不在が問題になってから」）。ただし「タグ形式は `<system-reminder>...</system-reminder>` で揃える」「履歴は汚さない」の2点は本実装で確立し、将来の追加機構が同じ規約に乗れるようにする
- **発火はナッジ型**。N リクエスト無アクティビティで初めて発火し、cooldown も持つ

## 要件

### Interceptor

- `pre_llm_request` で `Vec<Item>` を受け取り、以下の AND を満たした場合のみ発動
  - active Task（`pending` または `inprogress`）が1件以上存在する
  - 直近 N リクエスト (暫定 N=8) `TaskCreate` / `TaskUpdate` のいずれも呼ばれていない
  - 前回 reminder 注入から M リクエスト (暫定 M=8) 以上経過
- ここで言う「リクエスト」は **LLM への1回の推論呼び出し (= assistant 応答1回)** の単位で数える。ユーザー発火単位ではない。1ユーザー発火内で tool ループが回れば、tool_result を受けて発火する次のリクエストもそれぞれ1としてカウントする
- カウント対象はメインスレッドの assistant 応答に限る。サブエージェント / sidechain の assistant 応答は除外する
- カウンタは Pod 側の session-lifetime 状態として保持する（`requests_since_last_task_management` / `requests_since_last_reminder`）。resume 時は履歴の逆走査で再計算するか 0 リセットで再開する。どちらでも「初回ナッジが最大 N リクエスト遅れる」だけで挙動として致命ではない
- 発動時、直近の user message の content（または content[最終 text part]）の末尾に `<system-reminder>` ブロックを append し、現在の active Task リストを `taskid` / `status` / `subject` を含む簡潔な形式で列挙する。`description` は長大化を避けるため省略してよい
- 履歴 (`Worker` の保持する `Vec<Item>`) は変更しない。リクエスト送信時の Vec のみ加工する
- active Task が空の場合は何も差し込まない（忘却防止が目的なので、思い出させる対象が無いなら不要）

## 完了条件

- 直近 N リクエスト連続で `TaskCreate` / `TaskUpdate` が呼ばれず、かつ active Task が残っている場合に限り、`pre_llm_request` で `<system-reminder>` が直近 user message に append される
- `TaskCreate` / `TaskUpdate` のいずれかが呼ばれるとカウンタがリセットされ、再び N リクエスト経過するまでは reminder が出ない
- reminder が一度出たあとは、cooldown M リクエストが経過するまで再注入されない
- active Task が0件の場合は reminder が出ない
- system-reminder の注入は揮発的で、`get_history` / セッションログには現れない
- 単体テストで Interceptor の発火条件（リクエスト回数閾値、active 0件、cooldown、サブエージェント除外）がカバーされる

## 範囲外

- inprogress 滞留検出 / 多重 inprogress 検出など、状態異常ベースの追加トリガ（必要になれば別チケットで追加）
- system-reminder 注入機構の汎用化（`TODO.md` に立項済み、別途検討）
- `TaskCreate` / `TaskUpdate` の戻り値に active Task 全件を埋め込む強化（必要に応じて Tool ticket 側で対応）

## 参照

- 設計指針: `CLAUDE.md`（最小の構造化 / 概念の追加は不在が問題になってから）
- 前提: `tickets/session-todo.md`（Tool 群と TaskStore）
- 参考実装: Claude Code の `task_reminder`（local reminder helper functions、閾値 `a local reminder threshold`）

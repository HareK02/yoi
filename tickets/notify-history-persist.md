# 注入される system message をワーカー履歴に永続化する

## 背景

現状、`Method::Notify` および `Method::PodEvent`（`TurnEnded` / `Errored` / `ShutDown` / `ScopeSubDelegated`）は、親 Pod 側で `NotifyBuffer` に積まれ、`PodInterceptor::pre_llm_request` で **その1リクエスト限り**のsystem messageとして注入される（`crates/pod/src/ipc/interceptor.rs:147-159`）。

しかし `Worker::run` は `let mut request_context = self.history.clone();` してから interceptor を呼ぶ（`crates/llm-worker/src/worker.rs:862, 913`）ため、interceptor が push した notification は clone 側にしか乗らない。一方、その後の LLM 応答（`assistant_items`）は `crates/llm-worker/src/worker.rs:946` で **本体の `self.history`** に extend される。

結果、履歴上は「文脈ゼロから Pod がいきなり `ReadPodOutput` を呼んだ」「何の前触れもなく `child Pod が落ちた件について調べる』と発言した」などの状態になる。次のリクエスト時点で既に LLM 自身の自己一貫性が壊れており、コンパクション・再起動を待たずして破綻している。

同じ問題は `tickets/session-todo-reminder.md` で予定している `<system-reminder>` 注入にも存在する。当初「reminder は同じ内容を繰り返し注入する可能性があるから履歴を汚さない」という方針を取っていたが、これは category error だった:

- **キャッシュ破壊**: 揮発で last user message を mutate する設計だと、worker.history 側は元のままなので、毎回 reminder の有無/内容差分で「実際に LLM へ送る user message の content」が変動する。Anthropic prompt cache は anchor までしか効かないため、anchor 直後の生成が毎回 cache miss になる
- **LLM の自己一貫性**: turn N で reminder を見て `TaskUpdate` を叩いた → turn N+1 では reminder が消えて、自分の `TaskUpdate` 呼び出しだけ残る。Notify と全く同じ因果切断
- **resume 時の不整合**: ロードした history からは reminder が完全消失している状態で再開する
- **「繰り返し注入で履歴肥大」の前提も弱い**: cooldown 設計上 reminder は idle 期間に1回 + 反応で counter リセット。連発は元々しない。仮に複数回出ても、それぞれが「その時点での active Task の snapshot」として履歴に並ぶのは因果として正しい

つまり「LLM に投げた system message は、その時点で history に commit する」が原則で、Notify / PodEvent / `<system-reminder>` を一律にこの原則に揃える。

## 方針

- LLM リクエスト直前に注入される system message は、`request_context: &mut Vec<Item>` の clone 側ではなく **worker 本体の `history` 側** に append する
- `NotifyBuffer` は **「次のリクエスト直前で `worker.history` に append するキュー」** として再定義する
- 永続化（`history.json`）は worker.history 経由で自動的についてくる（`PodSharedState::history_json` → `RuntimeDir::write_history`）
- 対象は現状の `Method::Notify` / `Method::PodEvent` に加え、session-todo-reminder で予定している `<system-reminder>` 注入も含む（後者は前者と同じ NotifyBuffer に乗せるか別キューを立てるかは実装裁量。重要なのは「history に commit される」点）
- `notify_wrapper` の文言（`[Notification] ... not a blocking request`）はそのまま履歴に残してよい。後から見ても「これは ambient 通知だった」と分かる方が望ましい
- `<system-reminder>` も同様、タグ込みのまま history に残す（タグ形式 `<system-reminder>...</system-reminder>` の規約自体は維持）

## 要件

### Notify / PodEvent 経路の挙動変更

- `NotifyBuffer::drain` 由来の Item は `request_context` ではなく `worker.history` に append される
- append は **次の LLM リクエスト直前** に1回だけ起きる（複数 notify が貯まっていれば順序を保って複数 Item として並ぶ）
- append 後、`history.json` への永続化が通常の history mutation と同じパスで起きる
- 永続化された Item は次回 resume 時にそのまま履歴の一部として読み戻される

### `<system-reminder>` 注入経路（session-todo-reminder の前提変更）

- `<system-reminder>` ブロックを「直近 user message を mutate して append」する設計を撤回し、**新規 system message Item として `worker.history` に append** する形に変更する
- ライフサイクルは Notify / PodEvent と同じ: 注入条件を満たした時点で history に commit、`history.json` に永続化、resume 後も読み戻される
- session-todo-reminder.md 側の「履歴を汚さない」「`get_history` / セッションログには現れない」「last user message を mutate」記述は本ticketの方針で上書きする

### 注入レーンの統一

- 「LLM リクエスト直前に注入される system message」は一律 history レーンに乗せる、と `crates/pod/src/ipc/notify_buffer.rs` のモジュールdocに明記する
- 「揮発（history を汚さない）レーン」の概念は廃する。将来 reminder 系を追加する際も同じ原則に従う
- 命名・配置を見直す必要があれば実装内で判断してよい（例: `NotifyBuffer` を `PendingSystemMessages` 等に改称、reminder 用の別キューを作る等）。本ticketは挙動の正しさが最優先で、抽象の作り方は実装者裁量

### 既存テスト・ドキュメントの更新

- `crates/pod/src/ipc/interceptor.rs` の `pre_llm_request_drains_pending_notifies_into_context` 系テストは、`request_context` ではなく `worker.history` への反映を検証する形に書き換える
- `crates/pod/tests/pod_events_test.rs` で `PodEvent` 受信後に history に対応 Item が現れることを E2E に近い粒度で確認するケースを追加する
- 既存の「揮発レーン」前提のコメント（`crates/pod/src/ipc/notify_buffer.rs:5` の `(never into the Worker's persistent history)` 等）を新方針に合わせて書き換える
- `tickets/session-todo-reminder.md` の方針記述を本ticketの完了に合わせて更新する（または本ticket完了時点で先行修正してよい）
- `TODO.md` 末尾の「タグ形式と『履歴を汚さない』原則は session-todo で先行確立」記述から後者を撤回する

## 完了条件

- 親 Pod が `Method::Notify` または `Method::PodEvent` を受信すると、その後の最初の LLM リクエスト直前に対応 system message が **`worker.history` に append** され、リクエストにも含まれる
- 同じ Item が `history.json` に書かれており、`Pod::resume` 後に履歴の一部として読み戻される
- LLM が notification に反応して取った行動（tool call / 応答）と、そのトリガーとなった notification Item が、履歴上で因果順に並んでいる
- session-todo-reminder で導入される `<system-reminder>` 注入も同じく history append として実装される（または、実装順次第で本ticketは Notify / PodEvent 側だけ完了させ、session-todo-reminder 実装時にこの原則に従う形でもよい。後者の場合は session-todo-reminder.md 側の方針記述を本ticket完了時に更新済みであることが必須）
- 単体テストで上記が確認できる

## 範囲外

- `notify_wrapper` の文言・phrasing の見直し
- TUI 側の Notify / PodEvent 表示（`Event::PodEvent` 経路は既存通り）
- compaction 時の notify Item の扱い（通常 Item と同じく compaction 対象になればよい。特別扱いは不要）
- `<system-reminder>` 注入機構の汎用化（`TODO.md` の既存項目。本ticketは個別実装の方針統一だけ扱う）

## 参照

- 設計指針: `CLAUDE.md`
- 関連: `crates/pod/src/ipc/notify_buffer.rs`、`crates/pod/src/ipc/interceptor.rs`、`crates/pod/src/ipc/event.rs`、`crates/pod/src/controller.rs`（受信ハンドリング）、`crates/llm-worker/src/worker.rs:862, 913, 946`（clone する側）
- 方針反転対象: `tickets/session-todo-reminder.md`（「履歴を汚さない」前提を本ticketで撤回）

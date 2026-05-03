# Review: セッション内 Task ツールの注意機構

対象: `tickets/session-todo-reminder.md` (`28fe1da` で新規作成)。実装は未着手 (本レビュー時点でコード変更なし、worktree 内 grep で `task_reminder` / `requests_since_last_task` 等いずれもヒットせず)。

本レビューは spec 単独レビュー (実装が始まった時点で完了条件を再確認)。

## 前提・要件の確認 (spec の妥当性)

- 「`pre_llm_request` で `Vec<Item>` を受けて直近 user message に `<system-reminder>` を append する」は、`crates/pod/src/ipc/interceptor.rs` の既存 `pre_llm_request` レーンに自然に乗る。Worker history を変更せず、リクエスト送信用 `Vec<Item>` のみ加工する方針は、既存の `pending_notifies.drain()` と同じパターンで先例あり (`interceptor.rs:147-159`)。spec として実装可能性は高い。
- TaskStore からの active 件数取得には A の `TaskStore::list()` だけで足り、追加 API 不要。
- カウンタ (`requests_since_last_task_management` / `requests_since_last_reminder`) を Pod の session-lifetime 状態として持つ方針は、既存 `tool_calls_this_turn: AtomicUsize` (`interceptor.rs`) と同じ素地に追加できる。pre_tool_call で `TaskCreate` / `TaskUpdate` を観測したら片方をリセット、`pre_llm_request` で発火条件を見て両方を増やす、という骨格になる想定。spec の粒度は問題ない。
- メインスレッド限定 / sub-agent 除外: 主要 worker と spawn pod / compact worker を分離している現実装では「メインの `PodInterceptor` だけがカウンタを持つ」で達成できる。compact worker 側は別 Worker / 別 Interceptor。spec の前提は現実装と一致。
- resume 時のカウンタ復元: 「履歴の逆走査で再計算 or 0 リセット」のどちらでも実害無しと spec 側で割り切られているのは妥当。0 リセットの方が大幅にシンプルで、初回ナッジが最大 N 遅れるだけ。実装時はこちらを推奨したい (spec 側は両許容で問題なし)。
- 「揮発的 = 履歴を汚さない」「タグ形式 `<system-reminder>...</system-reminder>` で揃える」「2 件目の利用者が出た時に汎用化を検討」の 3 点を明文化していて、A と B の隙間に「汎用 reminder 機構」の中途半端な抽象が立たないようにしている。CLAUDE.md の「概念の追加は不在が問題になってから」と整合。

## アーキテクチャ・スコープ

- B は A に対する後続チケットとして適切に分離されている。前提依存 (TaskStore + Task* tools) を `## 前提` に明記しており、A 完了後に着手するという順序付けも自然。
- 範囲外 (inprogress 滞留 / 多重 inprogress / 注入機構汎用化 / Tool 戻り値の active 全件埋め込み) が列挙されており、本チケットで肥大化しないラインが引けている。
- 「Pod 側に session-lifetime カウンタを増やす」程度の侵襲で、既存の `crates/pod/src/ipc/interceptor.rs` 内に追加実装が収まる規模感。クレート構成や層境界に新しい歪みは生じない見込み。
- 「N=8 / M=8 暫定」と数値を明示しているのは判断しやすい。Claude Code が `a local reminder threshold` という参照値を持つことも書かれており、後で詰めやすい。

## 指摘事項

### Non-blocking / Follow-up (spec 段階の論点)

- **「リクエスト = LLM への 1 回の推論呼び出し」のカウント単位が、現 `Interceptor` の `pre_llm_request` 1 呼び出しと一致するか確認が必要。** 現実装の `pre_llm_request` は context 構築直前に必ず通るレーンで、tool ループ内の続行 LLM 呼び出しでも毎回呼ばれる。実装時に「pre_llm_request 突入時刻 = LLM へのリクエスト 1 件」が成立しているかを軽く検証するテストを1本入れてほしい (tool ループ中の発火 cadence のため)。
- **TaskCreate / TaskUpdate のカウンタリセット契機。** 現状 spec は「呼ばれていない」と書かれているのみ。実装ではどのフックで観測するかの選択 (`pre_tool_call` か `post_tool_call` か) を決めて spec に追記しておくと実装ブレが減る。`pre_tool_call` で名前判定 + リセット、で十分。
- **active Task の取り出し経路。** spec 上は明示されていないが、Interceptor が TaskStore handle を保持する形になる (Pod 経由で渡す)。`PodInterceptor::new` 等のコンストラクタに `Option<TaskStore>` を増やす想定で良いと思うが、実装時に `Tracker` の例 (`attach_tracker`) と同様、Pod から Interceptor への手渡しを統一してほしい。
- **resume 時の発火タイミング。** 0 リセットを採るなら、resume 直後の最初のリクエストでは出ない。これは spec 上「最大 N 遅れるだけ」と許容済み。実装時は単にこの選択を README/コメントに残してほしい。
- **`<system-reminder>` の本文。** spec は「taskid / status / subject を簡潔に列挙」と書かれているが、A 側の `render_snapshot` を流用するか、reminder 専用に別関数を切るかが未決定。短さを優先するなら専用 fmt が望ましい (description を必ず落とすため)。実装方針を spec に1行追記しておくと安全。
- **テスト範囲。** spec の完了条件にある「リクエスト回数閾値 / active 0件 / cooldown / サブエージェント除外」は単体テストでカバー可能。実装時はそれぞれ独立ケースで書いてほしい。

### Nits

- 数値 N / M が「暫定 N=8」「暫定 M=8」と明記されているが、変更可能性についてのフォローアップ条項 (例: 運用後に閾値を見直す) があると親切。
- Claude Code 側の参照値 `a local reminder threshold` は spec に書かれているので、Insomnia 側 8 を採る理由 (短めにして気付かせやすくする等) を一文添えると将来 tuning しやすい。

## 判断

**Approve (spec 段階)** — 前提・要件・範囲外の切り分けが妥当で、A の実装規約 (TaskStore handle、揮発的注入、タグ形式) に乗る形で実装可能。アーキテクチャ的歪みも生じない見込み。実装着手前に上記 Non-blocking 4 点 (カウンタリセット契機 / active 取得経路 / 本文 fmt / resume 時のリセット選択) を spec に1〜2行追記してから実装に入ると、実装中の判断ブレが減る。実装完了時に完了条件を再確認する必要がある (本レビューは spec 妥当性のみの判定)。

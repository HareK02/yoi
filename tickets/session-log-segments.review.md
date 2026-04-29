# Review: セッションログの Segment 保持

## 前提・要件の確認

- **`LogEntry::UserInput` から `item: Item` を取り除き `segments: Vec<Segment>` のみ持つ**
  満たされている。`session_log.rs:120` で置換済み。`collect_state` は `Segment::flatten_to_text` 経由で `Item::user_message(text)` を派生（`session_log.rs:254-258`）。
- **セッションログに Segment を persist し、`Pod` の入口で submit-time に直書き、`save_delta` は user_message を skip**
  満たされている。`session.rs:148-164` に `save_user_input` 関数追加。`session.rs:188-190` で `is_user_message()` を skip。`pod.rs:608-615` で `Pod::run` の入口（flatten 直前）に `save_user_input` を埋め込み、in-memory `user_segments` に push。
- **復元経路で client が segments を取り戻す（TUI で paste チップが復元）**
  実装済み。`ipc/server.rs:91-127` で `Method::GetHistory` 応答時に worker history JSON の user_message に `segments` フィールドを embed、`tui/src/app.rs:461-473` の `restore_history` で `segments` を読み出して `Block::UserMessage` を組み立てる。手動 UI 確認は未消化（report 申告通り）。
- **worker / llm-client 層は変更しない**
  守られている。`Segment::flatten_to_text` を `protocol` に追加した以外、worker / llm-client API に変更なし。
- **後方互換は持たない**
  既存 jsonl の読み込みフォールバックは入れていない（適切）。
- **Round-trip テスト**
  `replay_user_input_segments_round_trip` 追加（`session_log.rs:682-751`）。Text/Paste/FileRef を含む混合 segments の JSON 往復＋`flatten_to_text` 派生＋`user_segments` 保持を一気に検証。テスト粒度は要件十分。
- **既存ビルド・テストが新スキーマで合格**
  `cargo test --workspace` 全合格を確認。

## アーキテクチャ・スコープ

- `Segment::flatten_to_text` を `protocol` 側に純粋関数として抜き、Pod 側の `flatten_segments` を「アラート発火 + flatten_to_text 呼び出し」へ整理した分離（`pod.rs:636-682`）は正しい。replay 経路がアラートを再発火させない設計が両立している。
- session-store が `protocol::Segment` に依存する構造は妥当。`Cargo.toml` には `cargo add` で追加されており（コミットの差分通り）、依存方向（session-store → protocol）も既存の依存グラフ的に問題ない。
- `RestoredState.user_segments` を別フィールドとして並走させ、replay 経路では Item と Segment を二重管理する設計は、worker history の責務（LLM への渡し物）と client 復元の責務（typed atom 描画）の分離として理にかなっている。
- IPC server で `is_user_message()` 数と `user_segments` 数の差分から「seed history は最初に詰まれる」という性質を使った右寄せ整列（`ipc/server.rs:101-126`）は、ticket 記載の前提（"seed history は segments を持たない、live submission のみ持つ"）の素直な実装。

## 指摘事項

### Blocking

- **`Pod::compact()` 後に `self.user_segments` がクリアされない**（`pod.rs:1165-1224`）
  `compact()` は worker history を `[summary, ...retained_items]` に置換するが、`self.user_segments` は触られていない。プロセス継続中に compaction が走った場合、次の `Method::GetHistory` で `ipc/server.rs:103` の `total_user_msgs.saturating_sub(segments_per_user.len())` が想定外の値になり、segments と worker user_message が錯誤マッチする。
  例: 5 user_msg を投げた後 compaction で 1 だけ retained のとき、`total=1, segs=5 → skip=0` となり、retained の user_msg に 1 番目の古い segments が貼られる。
  対策案: `compact()` 内で `retained_items` 中の `is_user_message()` 件数 K を数え、`self.user_segments = self.user_segments.split_off(self.user_segments.len() - K)` 相当に切り詰める（K=0 なら clear）。controller 側 `shared_state` も同期するため、compact 後に `shared_state.set_user_segments(pod.user_segments().to_vec())` を呼び直す経路が要る。
  なお post-compaction 後にプロセスを再起動して restore する場合は、新セッションの jsonl に UserInput が無いので `state.user_segments` は空になり、自動的に整列が直る。問題は**プロセス継続中の compaction → 次の GetHistory** の窓に限定されるが、TUI の通常運用フローで踏みうる。

### Non-blocking / Follow-up

- **`save_user_input` 失敗時の shared_state ghost segment**（`controller.rs:331` と `pod.rs:608-615` の順序）
  Controller は `Method::Run` 受信直後に `shared_state.push_user_segments(input.clone())` してから run_future を await する。`save_user_input` が I/O エラーで失敗すると、`pod.user_segments` には push されないが `shared_state.user_segments` には残り、worker.history にも user_msg は積まれない（`save_user_input` の失敗で `pod.run` が早期 return するため）。次回 GetHistory で 1 段ずれる可能性。実害は store I/O 障害のレアケースに限定されるが、`run_future` 完了後に `shared_state.set_user_segments(pod.user_segments().to_vec())` で同期し直すのが安全。
- **seed history の segments 喪失**（`pod.rs:294-295`, `session_log.rs:217-223` のコメント参照）
  ticket の範囲外として明示済み・コメントでも記述されている設計判断。compaction を境に paste チップの典型 magenta 復元が単純テキストにフォールバックする挙動はユーザー視点で受容可能だが、UX として把握しておく価値あり。後続で必要なら `SessionStart.history` 側にも segments を持たせる拡張余地あり。
- **chain hash の確定性**
  `Segment` の serde 実装は HashMap 不使用・field 順序固定で、`session_log.rs:466-499` の `hash_chain_is_deterministic` / `different_content_produces_different_hash` で確認済み。スキーマ変更で hash 値は別物になるが、新スキーマ内での安定性は壊れていない（要件通り）。
- 手動 UI 確認（paste chip の magenta 復元）の自動テスト化は未消化。session-store + flatten 経路は単体テストでカバーされているが、TUI の `restore_history` 経路は serde_json::Value からの復元アサーションが望ましい follow-up。

### Nits

- `pod.rs:615` の `self.user_segments.push(input.clone())` は直前で `save_user_input` に `input.clone()` を渡しているため `input` を 2 回 clone している。`Vec<Segment>` の clone はそこそこ重い（特に Paste の content）ので、`save_user_input(... segments)` の引数を所有 `Vec<Segment>` のまま受けて push 後に消費するか、`save_user_input` を `&[Segment]` に変えてから push 一回にする選択肢。マイクロ最適化なので必須ではない。
- `ipc/server.rs:105-114` のコメント「seed user_messages always come first」は、`ensure_head_or_fork` の auto-fork 経路で発生しうる "seed + 一部 segments のあとさらに live segments" のケースまで明示してくれると将来の保守者に親切（実際には auto-fork 後も pod.user_segments は live 分のみ累積する形で右寄せが成立しているが、コメントを読むと一瞬不安になる）。

## 判断

**Request changes** — `Pod::compact()` 後に `self.user_segments` を切り詰めない件（Blocking）が、ticket の完了条件「現行の compaction / fork / restore のフローを壊さない」に直接抵触する。修正は数行＋shared_state 再同期で済む規模。それ以外の要件は満たされており、修正後は Approve 想定。

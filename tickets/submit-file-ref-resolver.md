# サブミット入力: FileRef リゾルバ

## 背景

`tickets/submit-tui-completion.md` で `@<path>` が typed atom として入力され、submit 時に `Segment::FileRef { path }` で Pod へ届く経路が完成した。一方 Pod 側 (`Pod::flatten_segments` in `crates/pod/src/pod.rs`) は今 `FileRef` を見ても resolver を持たず、`Segment::flatten_to_text` の placeholder (`[unresolved file ref: ...]`) を user message に inline するだけで、Warn alert を吐いて終わっている。

ClaudeCode の `@<path>` と同等の挙動 — submit 時にファイル本文を読み、LLM context にそのまま見せる — を入れる。`compact/worker.rs` の `mark_read_required` 経路で完成済の auto-read（`PodFsView::render_auto_read`）と兄弟関係になる、submit 時版のリゾルバ。

## 要件

### Item 配置

履歴に永続化する形は以下の **2 つの Item** にする:

```
[..., user_message, system_message(file1), system_message(file2), ...]
```

user message 自体は今と同じく `Segment::flatten_to_text` 由来のテキスト（`@<path>` トークンが残った placeholder 込み）。直後に `[File: <path>]\n<本文>` 形式の system message を、`FileRef` の出現順に追加する。次ターン以降も LLM が見える状態で残す（compact が走った時点で既存の auto-read 機構が引き継ぐ）。

inline 結合（user 1 メッセージに本文を流し込む）は採らない。

### 本文の取り扱い

- `PodFsView` (`crates/pod/src/fs_view.rs`) 経由で読む。スコープ判定は `ScopedFs` 任せ。
- 上限は通常の Tool Output と同じ `manifest::defaults::TOOL_OUTPUT_MAX_BYTES` (16 KB)。超過分は捨て、末尾に `[...truncated, <total> bytes total — use read_file for the rest]` を付ける。LLM が必要なら自分で `read_file` を呼ぶ前提。
- 非 UTF-8（バイナリ）はリゾルバが拒否する。後述の失敗扱いに倒す。

### 失敗時の扱い

スコープ外 / NotFound / バイナリ拒否は **Alert + placeholder 残置**:

- ユーザー向け Alert を `AlertLevel::Warn` で発火（理由を含めた一文）
- 該当 segment の system message は出さない（user message 中の `[unresolved file ref: <path>]` プレースホルダーがそのまま LLM に届く）

これは「ユーザーの誤入力を早期に可視化する」狙い。silent fallback にしない。

### Worker 側 API 拡張

submit 時に user message と system messages を一つの turn の前置として履歴に積む経路を、既存の `Interceptor` action-return パターンに合わせて足す。`TurnEndAction::ContinueWithMessages(Vec<Item>)` (`crates/llm-worker/src/worker.rs:903`) と同形:

- `Interceptor::on_prompt_submit` の戻り値を拡張し、`Continue` / `Cancel(String)` に加えて `ContinueWith(Vec<Item>)` を返せるようにする
- Worker の `Locked::run` は `ContinueWith` を受けたら user_item の push 直後に extras を `history.extend` する
- Hook (`crates/pod/src/hook.rs`) 側の戻り値（`PromptAction`）はこの拡張に乗せない。Hook は read-only な公開拡張面という設計（hook.rs:8-15 のコメント）を維持するため、Hook と Interceptor で戻り値型を分離する

### Pod 側の resolver 配線

- `PodFsView::resolve_file_ref(&self, path: &str, max_bytes: usize) -> Result<Item, ResolveError>` を新設。`ScopedFs` で読み、UTF-8 検証 + 16 KB 切詰めを行い `Item::system_message` を返す。エラーは `OutOfScope` / `NotFound` / `Binary` / `Io(io::Error)` を区別する
- `PodSharedState` に submit 中だけ使う stash (`Mutex<Vec<Item>>`) を一個追加。`pending_notifies` / `compact_state` と同じ流儀
- `Pod::run` で submit 直前に `Vec<Segment>` を走査して FileRef を resolver に通し、成功分は stash、失敗分は Alert に流す
- `PodInterceptor::on_prompt_submit` で stash を取り出して空でなければ `ContinueWith(items)` を返す

## 範囲外

- Knowledge / Workflow resolver（それぞれ `tickets/memory-phase2-consolidation.md` と `tickets/workflow.md` 側）
- 画像など binary attachment の typed メッセージ化（将来 `ContentPart::Image` 等を入れる別チケット）
- `@<path>:<line>-<line>` のような行範囲指定構文
- compact 後の auto-read との重複排除（compact が user message 由来の FileRef を読み直す可能性は許容）

## 完了条件

- `@<path>` を含む submit が、user message + 解決済み system message の 2 Item として履歴に残る
- 16 KB を超えるファイルは truncate され、その旨が LLM に見える形で示される
- スコープ外 / NotFound / バイナリは Alert として通知され、LLM 側は placeholder を見るのみ
- Hook の戻り値型は据え置き、Interceptor のみ `ContinueWith` を受け付ける
- 既存ビルド・テストを壊さない

## 依存

- `tickets/submit-tui-completion.md`（FileRef segment の wire 接続）

## 参照

- `crates/pod/src/pod.rs`（`flatten_segments`, `Pod::run`）
- `crates/pod/src/fs_view.rs`（`PodFsView` — auto-read の隣に置く）
- `crates/pod/src/ipc/interceptor.rs`（`PodInterceptor::on_prompt_submit`）
- `crates/pod/src/shared_state.rs`（stash 追加先）
- `crates/llm-worker/src/interceptor.rs`（`PromptAction` 拡張）
- `crates/llm-worker/src/worker.rs:903`（`TurnEndAction::ContinueWithMessages` 既存パターン）
- `crates/pod/src/hook.rs:8-15`（Hook と Interceptor の責務分離 doc）
- `crates/manifest/src/defaults.rs`（`TOOL_OUTPUT_MAX_BYTES`）

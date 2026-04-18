# Review: pod-comm-tools

実装コミット前レビュー。`cargo build -p pod` clean、`cargo test -p pod --test pod_comm_tools_test` 8/8 pass。

## 総評

チケット要件の中核（4 ツールの wire 動作、`SpawnedPodRegistry` の共有、scope 回収、stale reclaim トリガー）は達成。テスト構成もモックソケットで実 `pod` バイナリ非依存で速い。ただし以下のうち修正必須項目を解消してから完了にしたい。

## 指摘と判断

### 修正必須

#### 1. `SendToPod` が RUNNING 状態を検知しない（設計合意違反）

**状況**: `connect_and_send` は `Method::Run` を書いて即切断し、応答 event を読まない。Controller は RUNNING 中の `Run` に対し `Event::Error { code: AlreadyRunning }` を返すが、tool 側はそれを見ないので LLM に「送信成功」と返る。

**設計合意**: pod-comm-tools 議論の結論は「A. SendToPod は `Method::Run` を使い、Pod が IDLE でなければエラーを返す」。

**判断**: 修正必要。書き込み後に short timeout で 1 event 読み、`Error { AlreadyRunning }` を検知したら `ToolError::ExecutionFailed` に変換する。正常時は `TurnStart` など先頭 event を見て「受け付けられた」ことを確認して戻る。

### チケット文言と実装の整合

#### 2. Pod status (`running / idle / stopped`) の取得手段がない

**状況**: チケットは `ReadPodOutput`/`ListPods` 出力に `running / idle / stopped` を含めることを求めているが、`Event::History` にはステータスが乗っておらず、実装は「接続可=alive / 接続不可=stopped」の 2 値のみ。

**判断**: 今回のスコープからは外す。`Greeting` または別 event に `PodStatus` を載せる改修は `Method::GetHistory` のレスポンス設計変更を伴うので、別チケット化する。本チケット文言を `alive / stopped` に揃えて実装と一致させる。

#### 3. `StopPod` は終了確認を待たない

**状況**: チケット本文は「`Method::Shutdown` 送信 → 終了確認受信 → 切断」だが、実装は fire-and-forget。scope の明示 release と `scope_lock` 上の stale reclaim で整合性は担保されているので実害なし。

**判断**: チケット文言を「`Method::Shutdown` 送信（応答は待たない）」に修正。

### 範囲外として明文化

#### 4. `spawned_pods.json` からの再起動復旧は未実装

**状況**: `spawned_pod_registry.rs` のモジュールコメントに「today only write-through is implemented」と明記されている。spawner プロセス再起動後、`ListPods` は空リストを返す。

**判断**: 今回のスコープ外。チケットの範囲外に「spawner 再起動時の `spawned_pods.json` からの復旧」を明記し、必要になったタイミングで別チケットを切る。

#### 5. `ReadPodOutput` cursor の非永続性

**状況**: `SpawnedPodRegistry::cursors` は `HashMap<String, usize>` でインメモリのみ。spawner 再起動後、カーソルは 0 にリセットされ、既読だった assistant text が再送される。モジュールコメントに「cursors intentionally do not persist」と明記されている。

**判断**: 現状のままで OK。設計判断としてレビューで合意し、チケットの「設計で決めること」の回答として本 review に残す。チケット側には残さない（gitで追える）。

### 軽微

#### 6. `extract_assistant_text` のメッセージ境界

**状況**: 複数の assistant message を `\n` で連結するのみ。メッセージ境界が曖昧になる。

**判断**: `\n\n` に変更して境界を明確にする。修正コスト小。

#### 7. blocking `flock` を async から呼んでいる

**状況**: `release_scope` / stale reclaim 経路で `LockFileGuard::open`（内部で `flock(2)`）を async fn から直接呼ぶ。通常は瞬時だが厳密には tokio runtime を一瞬止める。

**判断**: 既存コード全体が同じ慣習なので今回は踏襲。気になったら別タイミングで `spawn_blocking` 化を検討。本チケットでは対応しない。

#### 8. `SpawnedPodRegistry::new` が `Arc<Self>` を返す

**状況**: コンストラクタが smart pointer を返す API は珍しい。

**判断**: consumer が全員 `Arc` で持つ設計なので許容。リファクタは不要。

## 完了条件との対応

| 完了条件 | 状態 |
|---|---|
| SendToPod で spawned Pod にメッセージ送信・Pod が処理 | ✅（指摘 #1 の修正後） |
| ReadPodOutput でカーソルベース差分取得 | ✅ |
| StopPod で graceful 停止 + scope 返却 | ✅ |
| ListPods で既知 Pod の状態一覧 + health check | ⚠️ `alive/stopped` のみ（指摘 #2） |
| 接続不可 Pod は `stopped` 扱い + stale 回収トリガー | ✅ |
| 各ツール正常系・異常系の単体テスト | ✅（8/8 pass） |

## 完了に向けた作業

1. 指摘 #1: `SendToPod` に応答確認を実装
2. 指摘 #2, #3: チケット本文の文言修正（`running/idle/stopped` → `alive/stopped`、Shutdown 応答待ちの記述削除）
3. 指摘 #4: チケットの「範囲外」に `spawned_pods.json` 復旧を追記
4. 指摘 #6: `extract_assistant_text` の区切りを `\n\n` に

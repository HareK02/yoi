# ネイティブ GUI クライアント MVP

## 背景

TUI は ratatui の `insert_before` を使った append-only モデルで動いており、ツール呼び出しのライブ更新（引数のストリーミングプレビュー、状態遷移の視覚化）のような「既に描いた領域の書き換え」が本質的に苦手である。`tickets/tui-tool-call-ui.md` で妥協的な拡張策（inline viewport を可変化してアクティブフレームを保持）は立てたが、terminal のスクロールバックモデルと live-updating な LLM UX の相性は根本的に悪く、どう組んでも制約と妥協がついて回る。

一方、GUI 側ならそもそも**全領域が毎フレーム再描画される retained-mode**なので、ツールフレームの live 更新・折り畳み・インタラクティブな介入といった操作が自然に書ける。TUI は軽量・ssh 親和のクライアントとして残し、**リッチな対話は GUI クライアントに切り出す**方針を取る。

## 方針

### アーキテクチャ

- **プロセス分離 + ソケット接続を維持**。GUI は独立バイナリとして動き、Pod はこれまで通り別プロセス。
- **通信は既存の `protocol` クレート**（`Method` / `Event`）をそのまま使う。GUI と TUI は同じ protocol を喋る。
- **Pod の spawn は GUI から直接行う**。manifest を選択 → `pod` バイナリを subprocess として起動 → その socket に接続、という流れ。daemon 層は導入しない。
- **MVP は単一 Pod**。複数 Pod の並列管理は本チケットの範囲外とし、GUI 側の protocol 抽象が固まってから別チケットで拡張する。

### GUI フレームワーク: GPUI

- Rust ネイティブ + async-aware で、`protocol` クレートを直接リンクできる。
- GPU 加速の retained-mode で、ツールフレームの live 更新が素直に書ける。
- virtualized list / text input / scrollable 履歴など、LLM チャット UI に必要な部品が一通り揃っている。
- 既知のリスク: プラットフォーム成熟度（macOS > Linux > Windows）、独立ライブラリとしての新しさ、Markdown レンダラ等のウィジェット生態系が Tauri/Iced より薄い。本 MVP は Linux only なのでプラットフォーム面のリスクは受容できる。

### プラットフォーム

- **Linux only**。macOS / Windows は MVP の範囲外。GPUI の Linux サポートが動作する前提で組む。

## MVP スコープ

### 含む

1. **Pod の spawn と接続**
   - manifest ファイルを選ぶ UI（ファイルダイアログ or CLI 引数）
   - `pod` バイナリを subprocess として起動し、その socket に接続
   - 接続確立後は TUI と同じ protocol で対話
2. **現 TUI の機能相当**
   - 入力フィールド + 送信
   - ストリーミングテキストの表示（`TextDelta` → 追記）
   - ターンヘッダ / ターン統計 / ステータスバー相当の情報表示
   - セッション再開時の履歴復元（`Event::History`）
   - エラー表示
   - cancel / graceful shutdown
3. **ツール呼び出しのフレーム更新 UI**
   - `tickets/tui-tool-call-ui.md` で定義したライフサイクル（Pending → Streaming → Executing → Done/Error）をそのまま GPUI 側で実装
   - TUI と違い inline viewport の制約が無いので、履歴スクロール内でも自由に再描画できる
   - `ToolCallArgsDelta` を毎フレーム反映してライブプレビュー
   - 完了済みフレームは履歴内に状態が焼き込まれた形で残る
4. **Pod の明示的 shutdown**
   - GUI から shutdown 操作を行い、Pod subprocess を graceful に終了させる
   - shutdown 完了後は GUI 自体も正常終了する

### 含まない

- 複数 Pod の並列表示・切替（別チケット）
- daemon 層の導入
- macOS / Windows サポート
- ツール結果のリッチレンダリング（Markdown 整形、シンタックスハイライト、diff 表示等）
- ツール実行への対話的介入（permission ask/reply の UI 実装は `tickets/permission-extension-point.md` 側）
- protocol の拡張（compact 通知・R-R パターン等は `tickets/protocol-design.md` 側で進行し、GUI は完了次第追従する）
- GUI 内での manifest 編集
- テーマカスタマイズ、キーバインドカスタマイズ

## 設計で決めること

- **GPUI のイベントループと Tokio ランタイムの統合**: GPUI 側の executor に Pod からの socket イベントをどう流し込むか
- **socket client の置き場所**: 現 `crates/tui/src/client.rs` と同等のクライアントを別 crate に切り出して共有するか、GUI crate 内に閉じて持つか
- **Pod subprocess のライフサイクル管理**: GUI プロセスが落ちたときの Pod 側の後処理（orphan prevention）、Pod が異常終了したときの GUI 側の復帰 UX
- **ツールフレームのデータモデル**: `OutputItem` 列に載せるか別コレクションで持つか（TUI 側の設計議論と共通部分あり。共有可能なら abstract して流用）
- **履歴スクロールの挙動**: 下端追従（chat 流儀）と手動スクロール時の追従停止
- **入力エリアの多行対応**: 単一行でよいか、複数行 + Ctrl+Enter 送信等

## 完了条件

- Linux 上で GUI バイナリが起動し、manifest を指定すると `pod` subprocess を起動して socket 接続する。
- 基本的なチャット（user 入力 → assistant 応答のストリーミング → ターン統計）が TUI と同等に動く。
- ツール呼び出しが 1 フレームとして表示され、`ToolCallArgsDelta` がライブでプレビューされ、完了時に視覚的に状態が変わる。
- セッション再開時に履歴（ツール呼び出し含む）が復元される。
- GUI から shutdown 操作で Pod を正常終了させられ、GUI 自体も正常終了する。

## 新規クレート構成（案）

- `crates/gui/` （GPUI を使うバイナリ）
- socket client を共有する場合は `crates/client/` を新設し、TUI と GUI がそれぞれ依存する形に整理する選択肢もある

## 範囲外（再掲・重要なもの）

- 複数 Pod の並列管理・切替。単一 Pod に集中する。
- macOS / Windows。Linux only で完結させる。
- protocol の新規イベント追加。既存 protocol で足りる範囲に留める。
- TUI の廃止。TUI は軽量クライアントとして並行して残る。

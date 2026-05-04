# TUI: セッションコンテキスト長 / ウィンドウ占有率の常時表示

## 背景

TUI の status line は現状、Run 単位の `↑net upload / ↓output` トークンしか出していない（`crates/tui/src/app.rs:55-61`、`crates/tui/src/ui.rs:850-895`）。これは「このターンで実際に課金される転送量」を示すには適切だが、「今このセッションがコンテキストウィンドウのどこにいるか」をユーザーが把握する手段がない。

Pod は `Event::Usage` で LLM 1 リクエスト分の `input_tokens`（cache 込みの prompt prefix 占有量）を毎回送ってきており（`crates/protocol/src/lib.rs:301-306`）、最新値がそのまま「現在のセッションコンテキスト長」になる。受信はしているが、TUI は cache 控除して run 集計に積むだけで保持していない。

ユーザーは compaction やプロンプト重複の発生を体感するために、コンテキスト消費を常に視認したい。

## 方針

3 段階の最小拡張で済ませる：

1. **TUI**: `Event::Usage::input_tokens`（cache 控除しない素の値）の最新値をセッション状態として持ち、status line に常時表示する。`Event::CompactDone` でリセット。
2. **Protocol**: `Greeting` に `context_window: u64` と `context_tokens: u64` を追加。前者はモデルのウィンドウ上限、後者は attach 時点の `Pod::total_tokens()` 推定値（次の Usage 到着まで TUI に表示できる初期値）。
3. **Manifest / Pod**: モデルのコンテキストウィンドウは provider catalog のモデルメタに常設の値として持たせ、必要なら manifest 側で override できる経路を 1 つ用意する。配置（catalog エントリの新フィールド or `ModelCapability` 拡張 or `[worker] context_window`）は実装着手時に確定。Greeting 構築時には必ず具体値が入る。

`Event::Usage` 自体は既存の wire を踏襲する。新イベントを足さない。

## 要件

- Run 中・Idle 中・Paused 中いずれでも status line に `ctx: <tokens> / <window> (<pct>%)` を表示する。
- セッション初期値: `Event::History` の `context_tokens` を採用。最初の `Event::Usage` 受信で上書き。
- 更新トリガ:
  - `Event::Usage::input_tokens` 受信ごとに最新値で上書き（cache 控除しない素の値）。
  - `Event::CompactDone` で 0 にリセット（次の Usage で上書き）。`Event::CompactStart` / `Event::CompactFailed` ではリセットしない。
- `Event::TurnStart` / `RunEnd` ではリセットしない（run 集計とは別のセッション単位指標）。
- 既存の run 集計（`request: N | ↑x/↓y | tool: ...`）は残す。`ctx:` は別フィールドとして併置する。

## 完了条件

- TUI 起動から終了まで、status line にコンテキスト消費とウィンドウ占有率が常時出ている。
- compact が走った直後、`ctx:` が一旦 0 に戻り、次のリクエストでセッション開始サイズが入る挙動が見える。
- カタログ未掲載モデルを inline で指定した場合でも、manifest 側 override で値を入れれば同じ表示になる。

## 範囲外

- 履歴ビュー側でのターンごとコンテキスト推移グラフ。
- 課金額（USD）換算表示。
- compact 閾値 / `compact_request_threshold` との対比可視化（threshold バー等）。本チケットは現値の表示までで、警告色の閾値運用は別チケット。
- pod_cli 等他クライアントの同等表示（TUI 限定）。

## 影響範囲

- `crates/protocol/src/lib.rs`: `Greeting` に `context_window: u64` / `context_tokens: u64` 追加。
- `crates/manifest` / `crates/provider/src/catalog.rs`: context window をモデルメタとして宣言する経路。実装着手時に置き場所を確定。
- `crates/pod/src/controller.rs`: `build_greeting` で window と `total_tokens()` を Greeting に詰める。
- `crates/tui/src/app.rs`: `App` に `session_context_tokens: u64` と `context_window: u64` を持たせ、`handle_pod_event` で更新。
- `crates/tui/src/ui.rs`: `draw_status` に `ctx:` フィールドを追加。

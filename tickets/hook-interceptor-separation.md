# Hook と Interceptor の責務分離

## レビュー状態

初回レビュー実施済み。[hook-interceptor-separation.review.md](hook-interceptor-separation.review.md) を参照。
要件全項目達成。**無条件で受け入れ可**。

## 背景

Worker の実行ループへの介入手段として `Interceptor` trait (llm-worker crate) と `Hook` trait (pod crate) が存在するが、現状では Hook が Interceptor と同じ権限を持っており、責務の分離が成立していない。

### 現状の構造

```
Worker
  └── Interceptor trait (llm-worker)
        ↑ implements
  HookInterceptor (pod)
        └── HookRegistry
              └── Vec<Box<dyn Hook<E>>>
```

- `Interceptor` は `&mut Vec<Item>` (context)、`&mut ToolResultInfo` (tool result) 等を直接受け取る。context の書き換え、history への干渉、tool result の改変が自由にできる
- `HookInterceptor` は `Interceptor` を実装し、各メソッドで Hook を順に呼ぶブリッジ
- `Hook<E>` の `call(&self, input: &mut E::Input)` が受け取る `E::Input` は `Vec<Item>` や `ToolCallInfo` **そのもの**
- **結果として Hook からも Interceptor と同じ全権限で context を操作できてしまっている**

### 問題

- Hook を将来スクリプト言語（Rhai, Wasm 等）に公開したとき、ユーザースクリプトが context を自由に書き換えられてしまう
- 内部機構（compaction トリガー、notification 注入、tool output truncation）と外部公開 hook が同じ権限レベルで動いており、何が安全に公開できるかが型で表現されていない
- Interceptor のドキュメントには「upper layers (e.g. Pod) implement」とあるが、Pod が直接実装するのではなく Hook 経由で間接実装しているため、設計意図と実態がずれている

## ゴール

Interceptor と Hook の責務を明確に分離し、型レベルで権限の境界を表現する。

## 方針

### Interceptor（内部実装用）

- **llm-worker crate に留まる**。Pod / Worker の内部機構が直接実装する
- context / history の**直接操作が可能**（`&mut Vec<Item>` 等を受け取る）
- 外部に公開しない。`pub(crate)` または Pod crate 内でのみ利用
- 用途:
  - compaction トリガー（`PreLlmRequest` で context サイズを見て `Yield` を返す）
  - notification 注入（`PreLlmRequest` で pending notifications を context に flush）
  - tool output truncation（`PostToolCall` で content を書き換え）
  - 将来の内部機構

### Hook（公開 API 用）

- **Pod crate で定義**。将来スクリプト言語やプラグインに公開する前提
- イベントの**観測**と**制御フロー判断**（continue / skip / abort / pause）のみ
- context の直接操作は**できない**。受け取るのは:
  - **読み取り専用の情報**（ツール名、引数の概要、ターン番号、etc.）
  - **制御フロー判断の返却値**（continue / skip / abort）
- 安全に sandbox 可能

### 実行順序

同じ決定点（例: pre_tool_call）で Interceptor と Hook の両方が登録されている場合:

```
[Interceptor: 内部機構の処理（context 操作等）]
  ↓
[Hook: 外部の判断（observe + continue/skip/abort）]
  ↓
Worker が次のステップに進む
```

Interceptor が先に走って context を整え、Hook はその結果を観測する。Hook の判断が Interceptor の操作を覆すことはない（abort 等で中断は可能）。

## 必要な変更

### Hook の Input 型を制限する

現状の `Hook<PreLlmRequest>` の Input は `Vec<Item>` だが、これを read-only のサマリ型に変更:

```rust
// Before: Hook が context を直接操作できる
impl HookEventKind for PreLlmRequest {
    type Input = Vec<Item>;       // &mut Vec<Item> が渡される
    type Output = PreRequestAction;
}

// After: Hook は read-only の情報だけ受け取る
impl HookEventKind for PreLlmRequest {
    type Input = PreRequestInfo;  // context のサマリ（item 数、token 推定等）
    type Output = PreRequestAction;
}
```

同様に各イベントの Input を「観測に必要な最小限の read-only 情報」に絞る。

### `ToolCallInfo` / `ToolResultInfo` の分離

現状は Interceptor と Hook で同じ `ToolCallInfo` を共有している。分離:

- **Interceptor 用**: `ToolCallInfo { call: &mut ToolCall, meta: ToolMeta, tool: Arc<dyn Tool> }` — call の書き換え可能
- **Hook 用**: `ToolCallSummary { name: String, arguments_preview: String }` — 読み取りのみ、tool instance へのアクセスなし

### HookInterceptor の解体

現在の `HookInterceptor`（Hook を Interceptor にブリッジする層）は不要になる。代わりに:

- Worker が Interceptor を呼ぶ（内部機構の処理）
- Worker が Hook を呼ぶ（外部判断の問い合わせ）
- これらは Worker の実行ループ内で**別々のステップ**として呼ばれる

Worker が Hook を直接知るか、Pod 層が Worker に Hook callback を注入するかは設計時に判断。

### llm-worker 側の変更

- `Interceptor` trait はそのまま（内部実装用として健全）
- Worker の実行ループに「Hook 呼び出しポイント」を追加（Interceptor とは別系統）
- Hook の呼び出しインターフェースは `Fn` ベースのコールバックか、新しい trait か

### pod 側の変更

- `Hook<E>` の `E::Input` を read-only サマリ型に置き換え
- `HookInterceptor` を削除
- 内部機構（compaction 等）は Interceptor を直接実装する形に移行
- HookRegistry は外部公開 API として残り、将来のスクリプト公開の基盤になる

## 設計で決めること

- **Hook の Input 型の具体設計**: 各イベントで何を read-only として渡すか（ツール名? 引数の先頭 N 文字? token 数?）
- **Worker が Hook をどう呼ぶか**: Interceptor と同じ trait 方式か、コールバック登録方式か、別の trait か
- **Interceptor の複数登録**: 現状 Worker は1つの Interceptor しか持てない。内部機構が複数（compaction + notification + truncation）になると、Chain of Responsibility 的に複数の Interceptor を合成する仕組みが要る
- **Hook の実行順序保証**: 複数の Hook が登録されている場合の評価順序と short-circuit 規則（現行と同じ「登録順 + 最初の non-Continue で打ち切り」を維持するか）
- **既存の Pod 層 hook ユーザーの移行**: compact_interceptor 等が現在 HookInterceptor 経由で動いている場合の移行パス

## 完了条件

- Interceptor は context / history の直接操作が可能な内部 API として存続し、compaction / notification 注入 / tool output truncation 等が Interceptor を直接利用する
- Hook は read-only のサマリ情報のみを受け取り、制御フロー判断（continue / skip / abort / pause）を返す公開 API になる
- Hook から `&mut Vec<Item>` や `&mut ToolCallInfo` 等の context 直接操作パスが**型レベルで存在しない**
- HookInterceptor ブリッジは削除される
- Worker の実行ループ内で Interceptor → Hook の順序で呼ばれ、それぞれの責務が分離されている
- 既存の Pod 層 hook（compaction トリガー等）が Interceptor 直接実装に移行し、動作が壊れていない
- 単体テストで Hook が context を操作できないことが検証される

## 他チケットとの関係

- **tickets/method-notify.md**: notification の context 注入は Interceptor の責務。Hook ではない
- **tickets/pod-orchestration.md**: spawned Pod のツール実行制御（permission）は Hook として公開しうる
- **tickets/permission-extension-point.md**: パーミッション制御は Hook の代表的ユースケース。ツール実行の approve/deny を外部から制御する
- **tickets/bash-tool.md**: Bash ツールの Permission 層は Hook (pre_tool_call) で実装される想定

## 範囲外

- **スクリプト言語バインディングの実装**: Hook を Rhai / Wasm に公開する仕組み自体は別チケット。本チケットは公開可能な型境界を作るところまで
- **Hook の動的ロード / アンロード**: 起動時に登録して freeze する現行モデルを維持。動的な Hook 管理は別チケット

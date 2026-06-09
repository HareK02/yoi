---
title: "セッション解析ツールを追加する"
state: 'inprogress'
created_at: "2026-06-09T03:25:33Z"
updated_at: '2026-06-09T07:00:26Z'
queued_by: 'workspace-panel'
queued_at: '2026-06-09T06:56:20Z'
---

## 背景

ドッグフーディングのセッションが十分に濃くなり、Worker / Pod の挙動を体感だけで調整するのが難しくなっている。永続化された session JSONL から安定した指標を抽出できる基盤が必要。

現時点の懸念:

- prune / compaction によってモデルが有用な文脈を失い、同じファイルを繰り返し読んでいる可能性がある。
- `Read` の繰り返しが、必要な検証なのか、避けられる文脈喪失なのかを観測したい。
- `Edit` の `old_string` / `new_string` が大きくなり、出力トークンを不必要に増やしている可能性がある。
- 広すぎる `Bash` / `Grep` / `Glob` / `Read` の結果が history / context を膨らませている可能性がある。
- 同じファイルへの編集 churn から、非効率な workflow や tool guidance の問題を見つけたい。

まずは再利用可能な解析ライブラリとして作り、その上に CLI surface を薄く載せる。挙動を「無駄」と断定するのではなく、観測事実と suspicious pattern を報告する。

## ゴール

専用 crate `session-analytics` を作り、session JSONL log を解析して tool / session metrics を抽出できるようにする。その後、薄い `yoi session analyze` CLI として公開する。

## 要件

### ライブラリ crate

- 専用の解析 crate を追加する。例: `crates/session-analytics`。
- 1つの session file を解析する再利用可能 API を提供する。将来的には複数 session の集約も可能にする。
  - 例: `analyze_session(path) -> SessionReport` または同等の API。
  - 大きい session file を不必要に全読みしなくて済むよう、可能な範囲で streaming parse する。
- TUI rendering や Pod runtime side effect から独立させる。
- 既存の session / log schema 型は適切に再利用してよいが、runtime crate が analytics に依存する形にはしない。

### 最初に抽出する metrics

- Tool usage summary:
  - tool name / kind ごとの呼び出し回数。
  - turn ごとの tool call 数。
  - failed tool call 数。
  - 検出可能なら、より具体的な tool があるのに `Bash` で file inspection しているような broad tool usage pattern。
- File read duplication:
  - file path ごとの repeated `Read`。
  - 同じ path + offset/limit の repeated `Read`。
  - repeated read の間に対象ファイルへの write/edit があったかどうか。
  - 観測可能なら compaction / prune event 後の repeated read。
- Edit / write churn:
  - file path ごとの `Edit` / `Write` 回数。
  - 同一ファイルへの repeated edit。
  - `old_string` / `new_string` / edit argument 全体のおおよその byte size。
  - 出力トークンを増やし得る大きな replacement argument。
  - `replace_all` の使用。
- Tool result size:
  - 取得可能な範囲で output bytes / lines。
  - truncated / saved Bash output indicator。
  - large `Read` / `Grep` / `Bash` / web result の観測。
- Context lifecycle correlation:
  - session log に存在する prune / compaction 関連 event を記録する。
  - その後の repeated read / tool call と相関を取る。ただし因果として断定しない。

### Report model

- 後続の CLI / TUI / report rendering に使える構造化 `SessionReport` を定義する。
- summary total と suspicious-pattern diagnostics の両方を含める。
- 最初の安定 interface は machine-readable output を優先する。
- default report では user input / file content の raw dump を避ける。
  - path、count、byte size、timestamp / turn index、必要なら hash を含める。
  - raw snippet / content は default では出さず、必要なら後続で明示 opt-in にする。

### CLI integration

- product binary に薄い CLI entrypoint を追加する。例:

```text
yoi session analyze <session-jsonl-or-session-id> [--json]
```

- 初期版は明示 file path 指定だけでもよい。
- 後続拡張候補:
  - `--latest`
  - `--pod <name>`
  - directory aggregation
  - Markdown / human summary output
- CLI は library crate を呼び、analytics logic を重複実装しない。

### 意味論と制約

- repeated read を自動的に悪いものとして扱わない。
  - mutation 後の再読や validation のための再読は適切な場合がある。
  - 事実と bounded suspicious diagnostics として報告する。
- 通常出力で raw session content を露出しない。
- 解析は read-only にする。
- session log は記録済み tool call / history の authority として扱う。ただし context prune の因果は、明示 event がない限り相関としてしか推定できない。

## 非目標

- この Ticket で Worker の pruning / compaction 挙動を変更すること。
- analytics 用の full TUI dashboard を作ること。
- metrics に基づいて prompt / tool guidance を自動修正すること。
- LLM を使って session content を意味的に要約すること。
- default report で secret や raw file content を露出すること。

## 受け入れ条件

- `session-analytics` crate が存在し、tool analytics に必要な current session JSONL entry を少なくとも parse できる。
- library が tool counts、repeated reads、edit/write churn、edit argument sizes、large/truncated result observations を含む structured report を返す。
- `yoi session analyze <path> --json` または同等の CLI が machine-readable output を出す。
- report は default で raw user/file content を避ける。
- tests が以下を cover する:
  - intervening edit あり/なしの repeated reads。
  - large edit argument sizing。
  - tool failure counting。
  - representative log entry がある場合の basic compaction/prune event correlation。
  - malformed / unknown log entry を bounded diagnostics で扱うこと。
- focused tests、`cargo fmt --check`、`git diff --check`、`target/debug/yoi ticket doctor` が通る。

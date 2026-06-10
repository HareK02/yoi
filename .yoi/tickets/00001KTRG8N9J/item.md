---
title: 'Project role profilesをbuiltin profilesへ移行する'
state: 'inprogress'
created_at: '2026-06-10T10:11:51Z'
updated_at: '2026-06-10T15:26:32Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-10T11:16:43Z'
---

## 背景

現在、この workspace の Ticket role 用 Profile は project-local な `.yoi/profiles/*.lua` と `.yoi/profiles.toml` で定義されている。

対象例:

- `.yoi/profiles/_base.lua`
- `.yoi/profiles/companion.lua`
- `.yoi/profiles/intake.lua`
- `.yoi/profiles/orchestrator.lua`
- `.yoi/profiles/coder.lua`
- `.yoi/profiles/reviewer.lua`

これらは Yoi の標準的な role behavior / feature policy として汎用性が高く、各 project が個別に持つより builtin profile として提供した方がよい。Lua Profile API も `global yoi`、`yoi.profile.import`、`yoi.profile.extend` へ拡張済みなので、builtin default を base にした role-specific builtin profile を表現できる。

この Ticket は、現状 project 定義になっている標準 role profiles を `resources/profiles` 配下の builtin profiles へ移し、project config は builtin selector を参照するだけにするための follow-up である。

## 要件

- Ticket role 用の標準 Profile を builtin profile として追加する。
  - `builtin:companion`
  - `builtin:intake`
  - `builtin:orchestrator`
  - `builtin:coder`
  - `builtin:reviewer`
- builtin role profiles は `resources/profiles` 配下で管理し、compile-time embedded resource として登録する。
- 既存 `builtin:default` と同じ Profile registry / resolver 経路で選択できるようにする。
- role profile は Lua Profile API の標準形を使う。
  - global `yoi` object を使う。
  - 必要なら `yoi.profile.extend("builtin:default", overrides)` などで default を base にする。
  - `require("yoi.*")` 前提にはしない。
- 現在 project-local profiles が持っている feature/tool policy を builtin profile へ移す。
  - Companion: direct implementation worker ではなく、Ticket/Pods/Task などの権限を持たない相談・状況把握 role とする。
  - Intake: Ticket intake workflow 用の権限境界を維持する。
  - Orchestrator: Ticket orchestration workflow に必要な権限を維持する。
  - Coder: scoped implementation worker として必要な権限を維持する。
  - Reviewer: review worker として必要な権限を維持する。
- `.yoi/ticket.config.toml` の role profile selector を project-local selector から builtin selector へ移行する。
  - 例: `profile = "builtin:intake"`
- `.yoi/profiles.toml` と `.yoi/profiles/*.lua` の扱いを整理する。
  - builtin 移行後に不要な project-local role profiles は削除するか、明示的な project override sample として残すかを判断し、理由を記録する。
  - 残す場合は builtin を override する project-local profile として意味が明確であること。
- user/project 側で同名 profile を上書き・追加できる既存 registry semantics を壊さない。
- Profile import/extend は raw Profile artifact レベルで行い、resolved Manifest や runtime-bound field を builtin role profile に混ぜない。
- LLM-facing prompt / workflow 文言は Profile 内や Rust code に直書きしない。
  - role behavior の文言は `.yoi/workflow` または `resources/prompts` の既存方針に従う。
  - Profile は model / feature / permissions / memory / web / scope などの configuration に留める。

## 受け入れ条件

- `target/debug/yoi profile` または既存の profile listing/showing 経路で、role builtin profiles が確認できる。
- `builtin:companion` / `builtin:intake` / `builtin:orchestrator` / `builtin:coder` / `builtin:reviewer` が resolver で解決できる。
- `.yoi/ticket.config.toml` の roles が builtin profile selector を参照している。
- Ticket role launcher が builtin role profiles を使って既存通り起動できる。
- 現在の project-local role profiles と同等の feature/tool policy が維持されている。
- builtin role profiles は `resources/profiles/default.lua` と同じく global `yoi` style で書かれている。
- project-local `profiles.toml` / `.yoi/profiles/*.lua` の残存有無について、実装報告で理由が説明されている。
- Profile validation により、builtin role profiles が runtime-bound field や concrete authority を含まないことが確認されている。
- manifest crate の builtin profile registry / resolver test が追加・更新されている。
- Ticket role config / launcher 側の targeted test が必要に応じて追加・更新されている。
- `cargo test -p manifest profile` または該当 targeted test が通る。
- `target/debug/yoi ticket doctor` が通る。

## 非目標

- role workflow 本体を builtin profile に埋め込むこと。
- LLM-facing prompt 文言を profile file に移すこと。
- project が独自 role profile を定義・override できる能力を消すこと。
- Profile registry / selector semantics の大規模再設計。
- runtime-bound `pod.name`、resolved path、concrete delegated `scope.allow` を builtin profile に持たせること。

---
title: 'Implement Agent Skills support'
state: 'inprogress'
created_at: '2026-07-15T19:43:46Z'
updated_at: '2026-07-15T23:01:28Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-15T21:26:15Z'
---

## 背景

Yoi は再利用可能な作業手順、作法、報告形式、補助資料を、LLM-facing な手順資源として扱う必要がある。これらは external state や authority を直接動かす仕組みではなく、Agent が必要な時に参照・活用する procedural guidance として定義する。

Agent Skills 標準に従い、workspace-local Skills は `.yoi/skills/` 配下の `SKILL.md` package として扱う。Skill discovery / lint / catalog / activation は Workspace backend を authority とし、Worker は Workspace API から Skill metadata / body を受け取る。Ticket、Worker、workdir、repository、network などの外部状態変更は typed feature/tool surface が持ち、Skill はその使い方や作業の進め方を記述する。

## 要件

### Agent Skills 形式

- Skill の workspace ディレクトリは `.yoi/skills/` とする。
- Skill は Agent Skills 標準に従い、各 Skill をディレクトリとして配置する。

```text
.yoi/skills/
  <skill-name>/
    SKILL.md        # required
    scripts/        # optional
    references/     # optional
    assets/         # optional
```

- `SKILL.md` は YAML frontmatter + Markdown body とする。
- `SKILL.md` frontmatter は少なくとも以下を扱う。
  - `name` required。
    - parent directory name と一致する。
    - 1-64 characters。
    - lowercase letters / numbers / hyphen のみ。
    - 先頭/末尾 hyphen と連続 hyphen を禁止する。
  - `description` required。
    - 1-1024 characters。
    - Skill が何をするか、いつ使うかを具体的に書く。
  - `license` optional。
  - `compatibility` optional。
  - `metadata` optional string map。
  - `allowed-tools` optional / experimental。実装可否は要決定事項とする。
- `SKILL.md` body は LLM-facing procedural guidance として読み込む。
  - 推奨内容: step-by-step instructions、input/output examples、edge cases、report shape。
  - main `SKILL.md` は大きくしすぎず、詳細は `references/` 等へ分ける。
- `scripts/` / `references/` / `assets/` は Skill root 相対 path で参照する。
  - `references/` は必要時に読む追加 docs。
  - `scripts/` は agent が実行し得る補助 code。
  - `assets/` は templates / static resources。
- Workflow projection のために作られた標準外の独自仕様は Skill schema から削除する。
  - Skill を `WorkflowRecord` に変換するための `model_invokation` / `user_invocable` 相当の概念を Skill frontmatter / catalog に持ち込まない。
  - workflow invocation や workflow graph 前提の field / semantics を Agent Skills として受理しない。
  - 既存 parser が workflow 互換のために受けている field がある場合、標準 field か Yoi の明示拡張として再分類し、不要なものは warning ではなく lint error または unsupported として扱う。
  - Agent Skills 標準外の Yoi 拡張を残す場合は、projection 由来ではなく独立した必要性を説明し、namespace / compatibility 方針を明示する。

### Workspace authority / API

- Skill discovery / lint / catalog / activation の authority は Workspace backend とする。
  - `.yoi/skills/` は tracked workspace data の保存場所であり、Worker が各自で filesystem scan する source of truth ではない。
  - Web Workspace UI、Runtime Worker、embedded Worker、CLI は Workspace backend API 経由で同じ Skill view を得る。
- Ticket backend API と同様に、Worker / Runtime process が Workspace backend 経由で Skill を扱える typed API を整備する。
  - Skill catalog/list endpoint。
  - Skill detail/read endpoint。
  - Skill lint/diagnostics endpoint。
  - Skill activation body endpoint。
  - references / assets の resource read endpoint、または backend-resolved authority を返す endpoint。
  - 必要なら Worker-facing Skill operation endpoint を用意し、Worker-local filesystem scan に依存しない。
- Workspace backend は Skill catalog API を提供する。
  - `name` / `description` / source / provenance / override status / diagnostics を返す。
  - invalid Skill は catalog diagnostics として報告し、Worker ごとにばらばらな parse behavior を持たせない。
- Workspace backend は Skill activation/read API を提供する。
  - `/skill-slug` activation は Workspace backend から full `SKILL.md` body を取得し、Worker history に append / commit してから LLM context に載せる。
  - Worker は `WorkspaceClient::Http` がある場合、Skill body を local file path から直接読まない。
- Local-only / legacy Worker path が必要な場合も、同じ Workspace skill loader logic を共有し、別 schema / 別 precedence を作らない。
- Workspace settings / Profile / role が advertised skills や default visible skill catalog を指定する場合も、最終的な解決は Workspace backend の catalog authority に寄せる。

### Progressive disclosure / loading

- Skill metadata は Worker initialization 時に Workspace backend から軽量 catalog として取得できるようにする。
  - model-visible な常時情報は `name` と `description` を中心にする。
- Full `SKILL.md` body は Skill が activate / select された時だけ Workspace backend から取得して LLM context に入れる。
- `references/` / `scripts/` / `assets/` の内容はさらに必要時だけ読む。
- Builtin skill と workspace skill の discovery / override / provenance を定義する。
  - workspace `.yoi/skills/<name>/SKILL.md` が同名 builtin skill を override できるかを明示する。
  - Skill catalog には source / provenance / override status を含める。

### Skill と他概念の境界

- Knowledge と Skill の境界を明確にする。
  - Knowledge は facts / rationale / reference。
  - Skill は task execution guidance / reusable procedure / report shape。
- Plugin / feature との境界を明確にする。
  - Skill は prompt/resource であり、外部状態を直接制御しない。
  - Ticket / Worker / workdir / queue / repository / network などの authority と状態変更は typed feature/tool surface が持つ。
- Coder review cycle や Orchestrator queue consumption は、Skill + Workspace/Ticket/Worker feature tools の組み合わせで設計する。
- 既存 role prompts / internal prompts / docs は Skill の利用を前提に整理する。

### Validation / tooling

- Skill lint を追加する。
  - `SKILL.md` frontmatter required fields / naming convention / parent directory match を検証する。
  - unknown / unsupported fields は方針に従って warning または error にする。
  - relative file references の安全性を検証する。
- Skill discovery tests を追加する。
  - builtin skill loading。
  - workspace `.yoi/skills` loading。
  - override priority。
  - provenance reporting。
  - invalid Skill diagnostics。
- Workspace Skill API tests を追加する。
  - catalog/list が Workspace settings と `.yoi/skills` を authority として使うこと。
  - detail/read/activation body が同じ Skill resolution を使うこと。
  - lint diagnostics が Worker / Web / CLI で共有できる response schema になること。
  - Worker-facing API 経由で Skill activation でき、Worker-local scan と結果が分岐しないこと。

## 参照

実装中に Agent Skills 標準や harness 側裁量を確認する必要が出た場合は、以下を参照する。

- Agent Skills home: https://agentskills.io/home
- Agent Skills specification: https://agentskills.io/specification
- Client implementation guide: https://agentskills.io/client-implementation/adding-skills-support
- Skill creation best practices: https://agentskills.io/skill-creation/best-practices
- Reference implementation / validator: https://github.com/agentskills/agentskills/tree/main/skills-ref

## 要決定事項

- Skill activation surface。
  - `/skill-name` のような slash command を持つか、Skill selection / activation tool にするか。
- Skill catalog を model-visible にする方法。
  - 起動時 prompt に metadata 一覧を常時載せるか、Workspace SkillList / SkillRead API-backed tools で探索させるか。
- `allowed-tools` の扱い。
  - Agent Skills 標準では experimental なので、Yoi で authority として尊重するか、lint metadata として読むだけにするか。
  - 尊重する場合、既存 feature/tool permission とどう合成するか。
- `scripts/` の実行 authority。
  - Skill bundled script を通常 Bash / tool authority で実行するだけにするか、Skill script 実行専用 tool を作るか。
  - script dependencies / shebang / executable bit / sandbox の扱い。
- Skill file access。
  - `references/` / `assets/` を Workspace Skill resource API 経由で読むか、通常 `Read` に Workspace backend-resolved path authority を渡すか。
  - deeply nested reference chain をどこまで許すか。
- Builtin skill の配置場所。
  - `resources/skills/<name>/SKILL.md` とするか、別の runtime resource layout にするか。
- Workspace override policy。
  - workspace skill が builtin skill を完全 override するか、同名 conflict を error にするか。
- Profile / role との接続。
  - Profile で default active skills / advertised skills を指定するか。
  - Orchestrator / Coder / Reviewer roles にどの Skill catalog を見せるか。
- Web Workspace UI。
  - `.yoi/skills` の一覧 / validation / edit を Web で扱うか、初期実装では backend / CLI のみにするか。

## 受け入れ条件

- `.yoi/skills/<skill-name>/SKILL.md` が Agent Skills 標準の required frontmatter と naming rules で Workspace backend によって lint される。
- builtin/workspace Skill の loading、override priority、provenance、lint が Workspace backend API tests で確認されている。
- Ticket backend API と同様に、Worker / Runtime process / Web / CLI が共有できる Skill catalog/read/lint/activation API が整備され、tests で確認されている。
- Worker / Web / CLI が同じ Workspace backend Skill catalog を参照し、Worker-local filesystem scan が primary authority になっていないことを tests で確認している。
- Full `SKILL.md` body は activation 時にのみ Workspace backend から取得されて LLM context に入り、metadata catalog と progressive disclosure の分離が tests で確認されている。
- Workflow projection 由来の標準外 Skill fields / semantics が Skill schema から除去され、Agent Skills 標準 field と明示的な Yoi 拡張だけが受理されることを tests で確認している。
- role prompts / internal prompts / docs が Skill 前提に更新されている。
- Ticket queue、Worker spawn、workdir 管理などの外部状態制御は Skill ではなく feature/tool surface に残る。
- Skill implementation decision points のうち初期実装で決めないものは、明示的に unsupported / ignored / warning として扱われる。
- affected crates の `cargo test` と `nix build .#yoi` が通る。

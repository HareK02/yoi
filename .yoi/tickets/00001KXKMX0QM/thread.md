<!-- event: create author: "yoi ticket" at: 2026-07-15T19:43:46Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: comment author: hare at: 2026-07-15T19:56:41Z -->

## Comment

## Current implementation check: Skills support

現行の Skills support は存在するが、first-class Skill ではなく **Workflow ingestion の一部**として実装されている。

### 既にあるもの

- `crates/workflow/src/skill.rs` に `SKILL.md` parser がある。
  - `<skills-root>/<name>/SKILL.md` を読む。
  - YAML frontmatter + Markdown body を要求する。
  - `name` / `description` は required。
  - `name` は parent directory name と一致する必要がある。
  - `name` は既存 `Slug` validation を通る必要がある。
  - `description` は non-empty かつ `WORKFLOW_DESCRIPTION_HARD_CAP` 以内。
  - `license` / `compatibility` / `metadata` / `allowed-tools` は parse 上は受理する。
  - unknown frontmatter fields は warning で無視する。
  - `allowed-tools` は warning を出すだけで enforcement しない。
- `load_skills_from_dir(root)` がある。
  - root 直下の directory だけを見て、各 `<name>/SKILL.md` を読む。
  - broken skill は warning で skip し、siblings は読み続ける。
  - missing root は empty 扱い。
  - deterministic order で読み込む。
- `manifest` に `[skills] directories = [...]` がある。
  - path は manifest/profile base から resolve される。
  - validation 後は absolute path required。
  - profile/manifest merge で directories は extend される。
- Worker startup で `[skills].directories` が scope read allow に追加される。
  - skill root は recursive read allow になる。
  - `SKILL.md` / `scripts/` / `references/` / `assets/` 全体を通常 Read できる。
- Worker startup で Skills が workflow registry に取り込まれる。
  - `SkillRecord::into_workflow_record()` により Skill が `WorkflowRecord` に変換される。
  - `model_invokation = true`、`user_invocable = true`。
  - つまり Skill は `/<slug>` で呼べる Workflow として扱われる。
- shadowing support がある。
  - internal/builtin/workspace Workflow が同 slug を持つ Skill より優先される。
  - 複数 skill directories 間では first-fed wins。
  - shadowed Skill は Worker notification に `[Skill shadowed] ...` として流れる。
- tests は parser / directory loading / Workflow projection / scope allow / ingest_skills をある程度カバーしている。

### 現行の限界

- Skill は first-class ではなく、WorkflowRecord へ projection されている。
  - Skill catalog / Skill activation / Skill provenance は Workflow registry に埋もれる。
  - Workflow 削除 Ticket と衝突する構造になっている。
- `.yoi/skills/` は implicit workspace Skill root ではない。
  - 現在は `[skills].directories` に明示された root だけを読む。
  - default workspace path として `.yoi/skills` を probe しない。
- builtin skills はない。
  - `resources/skills/<name>/SKILL.md` のような bundled layout は未定義。
- Progressive disclosure は不完全。
  - metadata は resident workflow entry として出るが、Skill catalog と full body activation が分離されていない。
  - full `SKILL.md` body は workflow invocation body として扱われるだけ。
  - `references/` / `scripts/` / `assets/` はただ read-scope に入るだけで、Skill-aware access/tooling はない。
- `allowed-tools` は authority に反映されない。
  - parser は認識するが warning のみ。
  - feature/tool permission と合成しない。
- frontmatter optional fields は保持されない。
  - `license` / `compatibility` / `metadata` は parse acceptance のみで、SkillRecord / catalog には残らない。
- lint surface が独立していない。
  - parser tests はあるが、`yoi skill lint` のような user-facing lint/check はない。
- Skill resource safety は最小限。
  - skill root を recursive read allow にしているだけ。
  - relative references / nested reference chain / script execution policy は未実装。
- Web Workspace UI / backend API はない。
  - `.yoi/skills` の list / validation / edit surface は未実装。
- current docs/comments は Skill を Workflow ingestion として説明しており、今後の方針と逆。
  - `crates/workflow/src/skill.rs` の module docs も「ingests it as a Workflow」と明記している。

### 現在地まとめ

現在の Skills support は **Agent Skills parser + manifest-configured directory ingest + Workflow compatibility projection** まで。Agent Skills 標準の directory shape と basic frontmatter validation は部分的にあるが、Yoi の Skill としての lifecycle / catalog / activation / provenance / lint / UI / authority boundary は未実装。

この Ticket の実装では、まず Workflow projection から切り離して、`.yoi/skills/<skill-name>/SKILL.md` を workspace Skill authority として扱う first-class Skill loader/catalog へ移す必要がある。


---

<!-- event: decision author: hare at: 2026-07-15T20:43:42Z -->

## Decision

Workspace backend を Skill discovery / lint / catalog / activation の authority とする。

`.yoi/skills/` は tracked workspace data の保存場所だが、Runtime Worker が各自で直接 scan して別々の Skill view を持つ構造にはしない。Web Workspace UI、Runtime Worker、embedded Worker、CLI は Workspace backend API から同じ catalog / diagnostics / activation body を取得する。

これにより、Skill の override/provenance/lint diagnostics、Web からの編集・検証、Worker への progressive disclosure を同じ authority に寄せる。


---

<!-- event: decision author: hare at: 2026-07-15T21:02:33Z -->

## Decision

Skill support should mirror the Ticket backend direction: Workspace backend owns the shared authority and exposes typed APIs for Worker / Runtime process / Web / CLI access.

Required API surface includes catalog/list, detail/read, lint/diagnostics, activation body retrieval, and resource access or backend-resolved authority for references/assets. Worker-local filesystem scanning must not become the primary authority when `WorkspaceClient::Http` is available.


---

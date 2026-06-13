## Resolution

ユーザー指示により close する。

この Ticket は legacy migration 由来の「内部 Worker / 内部 Pod を Workflow と同一仕様で扱う」構想だったが、現在の設計では Workflow と Prompt resource / internal prompt は別 boundary として整理されている。

- public builtin workflow / Yoi dogfood workflow の分離、`resources/workflows/<slug>.md`、workspace override、builtin provenance は関連 Ticket で対応済み。
- internal prompt は `resources/prompts/internal/*` と `PromptCatalog` / `resources/prompts/internal.toml` 側の責務として扱う。
- 元の要件に残る `INSOMNIA`、`.insomnia/workflow`、旧 `tickets/*.md` 前提は current Yoi 設計と一致しない。

したがって、この Ticket は実装 routing せず、退役 / superseded として完了扱いにする。将来、internal prompt の remaining gap や internal Workflow substrate が必要になった場合は、現在の Prompt resource / Workflow boundary を前提にした別の concrete Ticket として作成する。
# Workspace Web

Workspaceの状態確認と操作を担当するstatic SPA。Backend projectionを表示し、認可と永続化はBackendへ委ねる。独立したWorkspace／Worker registryは持たない。

## 実装の入口

- `src/routes/+layout.svelte`: application shellとHeader／Sidebarの枠。
- `src/lib/workspace/sidebar/`: scope別navigationとoverride stack。
- `src/lib/workspace/console/`: Worker transcriptと入力編集。
- `src/lib/workspace/drive/`: Drive通信とdraft／receipt管理。
- `src/lib/workspace/markdown/`: 共有Markdown rendererとリンク検査。
- [`src/lib/generated/`](src/lib/generated/README.md): Rust契約からの生成物。

## 維持規則

`src/app.css` はfont、cascade layer、semantic token、resetと基礎typographyを所有する。画面layoutと機能別styleは所有CSSまたはSvelte componentに置き、共通tokenを使う。

生成契約は手で編集しない。通常のWeb build／devもSvelteKit生成物を書き換えるため、稼働中のcheckoutを検証専用buildに使わない。

視覚・操作の共通規則は[Design Language](../../docs/principle/design-language.md)、製品固有の画面構成と操作は[UX設計](../../docs/principle/ux-design.md)、画像と実装条件の検証は[目視確認の規則](../../docs/development/ui-ux/visual-review.md)に従う。

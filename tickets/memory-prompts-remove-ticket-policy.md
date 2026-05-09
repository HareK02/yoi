# Memory prompts: project-specific ticket shadow policy を外す

## 背景

現在の bundled memory prompts は、INSOMNIA 自身の開発運用に由来する「ticket / TODO / worktree / commit は git が正本なので memory に記録しない」という規則を、一般ユーザーの workspace にも適用している。

特に `resources/prompts/internal/memory_extract_system.md` と `resources/prompts/internal/memory_consolidation_system.md` には、ticket file 作成・編集、TODO 更新、ticket 名、worker Pod spawn for ticket などを memory から落とす指示が明示されている。しかし ticket はこのプロジェクト固有の管理手法であり、insomnia の利用者に押し付けるほど汎用的・成熟した概念ではない。

このため、memory がユーザー workspace の実際の管理対象や作業文脈を過剰に捨てる可能性がある。デフォルト prompt は、INSOMNIA リポジトリ固有の ticket 運用ではなく、より一般的な「既存の正本をそのまま重複保存しない」程度の方針に留めるべきである。

## 要件

- bundled memory prompts から、INSOMNIA プロジェクト固有の ticket / TODO 運用を前提にした禁止文言を削除する
  - `ticket-file creation / edit`
  - `TODO updates`
  - `ticket Y was created`
  - `ticket file names`
  - `worker Pod was spawned for Z` が ticket 前提で語られている箇所
- `git is authoritative` の扱いを、一般ユーザーに押し付けない表現へ弱める
  - git / VCS に残る単なるファイル操作ログを memory が逐語的に shadow しない、という原則は残してよい
  - ただし project management 上の出来事や管理ファイルの内容が、将来の作業判断に効くなら memory へ抽象化して残せる余地を残す
- Phase 1 extract prompt と Phase 2 consolidation prompt の両方を整合させる
- 必要なら `docs/plan/memory-prompts.md` または `docs/plan/memory.md` に、default prompt は特定プロジェクトの ticket 運用を前提にしないことを明記する
- INSOMNIA 自身の運用で ticket shadow を避けたい場合は、bundled default ではなく workspace/user prompt override 側で表現できる状態にする

## 範囲外

- memory の file format / linter / Phase 1 / Phase 2 実行機構の変更
- 使用頻度メトリクスや Knowledge 化 gate の実装
- INSOMNIA プロジェクト自身の ticket 運用の再設計
- prompt override 機構そのものの変更

## 完了条件

- `resources/prompts/internal/memory_extract_system.md` から ticket / TODO 固有の shadow 禁止が消えている
- `resources/prompts/internal/memory_consolidation_system.md` から ticket / TODO 固有の shadow 禁止が消えている
- 代替文言が、特定管理手法ではなく「既存の正本と重複しない」「将来の判断に効く抽象だけ残す」という一般原則になっている
- default prompt が、ユーザー workspace の project management 手法を固定しない
- 既存テスト / prompt catalog 検査が通る

## 参照

- `resources/prompts/internal/memory_extract_system.md`
- `resources/prompts/internal/memory_consolidation_system.md`
- `docs/plan/memory-prompts.md`
- `docs/plan/memory.md`

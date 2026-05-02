# Manifest: Tool Output / File Upload 上限の分離とデフォルト緩和

## 背景

現在、tool result の本文上限は `manifest::defaults::TOOL_OUTPUT_MAX_BYTES` に集約され、`worker.tool_output.default_max_bytes` として manifest から設定できる。一方で submit 時の `FileRef` 添付（`@<path>` を `[File: <path>]` system message に展開する経路）も同じ `TOOL_OUTPUT_MAX_BYTES` を直接使っており、upload / attachment 用の上限として独立して設定できない。

このため、tool output の安全な truncation と、ユーザーが明示的に添付したファイル本文の取り込み量を別々に調整できない。また現在の既定値 16 KiB は、ファイル添付・tool output の双方で実運用上やや厳しい。

## ゴール

Tool Output と submit 時 FileRef upload / attachment の上限を manifest でそれぞれ設定できるようにし、既定値を現在の 16 KiB より緩和する。

## 要件

- Tool Output の上限は引き続き manifest から設定できること
- FileRef upload / attachment の上限を Tool Output とは別の manifest field として設定できること
- FileRef resolver は hard-coded な `manifest::defaults::TOOL_OUTPUT_MAX_BYTES` ではなく、解決済み manifest の upload / attachment 上限を使うこと
- Tool Output と FileRef upload / attachment の既定値を、現在の 16 KiB から引き上げること
  - 正確な値は実装時に決めてよいが、docs / tests / manifest defaults の説明と一致させること
- manifest cascade / overlay / serde default のいずれの経路でも同じ既定値・同じ field semantics になること
- 既存 manifest で新 field が未指定の場合は、新しい既定値で動作すること
- 既存の per-tool override の挙動を壊さないこと

## 完了条件

- Tool Output と FileRef upload / attachment を別々に manifest で設定できる
- FileRef upload / attachment の truncate テストが、新 field の値を使うことを検証している
- Tool Output の既存テストが、新しい既定値・既存 override semantics に合わせて更新されている
- `docs/pod-factory.md` など manifest 設定のドキュメントが更新されている
- 16 KiB を前提にしたコメント・テスト値・ドキュメントが残っていない

## 範囲外

- 正確な token counting による上限管理への移行
- UI 側で添付ファイルサイズを事前表示・警告する機能
- compact / auto-read の token budget 設計変更

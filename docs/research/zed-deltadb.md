# DeltaDB 調査メモ（Zed Industries）

調査日: 2026-05-01
出典: Zed 公式ブログ「Sequoia Backs Zed's Vision for Collaborative Coding」を中心に、二次情報・関連記事を補強。

## 1. 何か

DeltaDB は Zed Industries が開発中の **operation-based version control system / synchronization engine** である。Zed エディタの Series B（Sequoia Capital 主導、$32M）と同時に発表された、同社の次フェーズの中核プロダクト。

ひとことで言うと「Git の commit ベースを置き換えるのではなく、**コミット間の "あらゆる編集操作"** を粒度として保持する VCS」。CRDT を用いてリアルタイムに変更を記録・同期し、Git と相互運用可能な設計を採る。

> "DeltaDB ... uses CRDTs to incrementally record and synchronize changes as they happen ... designed to interoperate with Git" — Zed Blog

## 2. 動機（解こうとしている問題）

LLM／AI エージェント時代のコーディングで、Git の粒度が粗すぎることが課題と捉えられている。

- **会話／意図がコードから剥がれる**: PR コメントやチャットでのやり取り、AI への指示・修正・pivot は、コードが変わると参照先を失い、文脈ごと失われる。
- **スナップショットでは追えない**: Git は commit という離散点しか持たない。エージェントとの "continuous dialogue" や、commit 未満の編集ステップを履歴として扱えない。
- **同時編集の衝突**: 複数の AI エージェント＋人間がリアルタイムに編集する時、従来の merge conflict モデルは機能しにくい。
- **コード位置の永続参照**: コードがリファクタや改名で移動するたび、URL 形式のパーマリンクや議論の固定ピンが切れる。

これらは OpenAI の Sean Grove が指摘した「進化する仕様・プロンプトをどう track するか」という課題感とも重なるとされている。

## 3. 技術的な構成要素

### 3.1 Operation-based design（vs. snapshot-based）

- Git の **commit = snapshot** に対し、DeltaDB は **operation = 編集 1 個** を一次データとして保持する。
- 「every operation, not just commits」を track する。
- スナップショットは operation log から導出可能な派生物として位置付けられる（VCS 系では "operational" / "patch theory" 系の系譜：Pijul, Fossil, darcs と同じ家系の発想）。

### 3.2 CRDT による同期

- 並行編集の整合性を CRDT（Conflict-free Replicated Data Types）で取る。
- これは Zed エディタの multiplayer editing で既に実戦投入されている技術の延長：
  - **Logical location**: 編集をオフセットではなく `(insertion id, offset)` のアンカーで表現し、操作を可換にする。
  - **Replica ID + sequence number**: 中央が replica id を一度割り当てれば、以降は各 replica が独立に一意 ID を生成可能。
  - **Tombstone**: 削除はテキストを物理削除せず墓標として残し、論理位置解決を保つ。
  - **Lamport timestamp / vector timestamp**: 因果順序を尊重し、並行挿入の可視性を制御。
  - **Per-user undo map**: 単一スタックではなく操作 ID → カウントの map で、ユーザーごとに独立 undo を実現。
- DeltaDB はこの editor 内 CRDT を、**ファイル横断・リポジトリ規模・永続化**にスケールさせるレイヤと読み解ける。

### 3.3 Character-level permalink

- 「あらゆるコード変換を生き延びる文字単位のパーマリンク」を提供する。
- スナップショット時刻のファイル＋行番号ではなく、CRDT のアンカー（insertion id ベース）に対して URL 的な参照を発行できるため、リネーム・リフォーマット・移動でも切れない。
- ユースケース: 議論／レビュー／AI への指示／設計メモを、特定の文字位置に永続的に固定する。

### 3.4 Git との相互運用

- リプレースではなく interop 前提。
- 既存 Git リポジトリを保ったまま段階導入できる戦略で、エンタープライズ採用のハードルを下げる狙い。
- 推測: operation log → Git commit へのフラット化／ Git commit → operation log への lift が可能な層を持つはず（公式の実装仕様は未公開）。

## 4. Zed エディタとの関係

- DeltaDB は Zed の中で「人間 × 人間」「人間 × AI エージェント」「AI × AI」の協働基盤になる。
- Zed が既に持つ multiplayer editing（CRDT）を、**editor session の寿命を超えて永続化・分散同期**する位置づけ。
- 「terminal interface、local IDE、web-based agent tool」の 3 系統の良さを束ねた統合 GUI を作る、という Zed の方針の中核。
- 「コードベースを生きた、辿れる履歴（a living, navigable history）として扱う」というビジョンの実装手段。

## 5. ビジネス／ライセンス

- Zed 本体と同様に **オープンソース＋ optional paid managed service** モデル。
- Series B（$32M、Sequoia 主導）の調達は DeltaDB 開発を主目的の一つとしている。報道では累計 $42M 規模との表記もある。
- 競合として GitHub と比較する論調も出てきている（例: Hypeburner が "GitHub Competitor" と表現）。ただし公式は「Git と interop」を強調しており、置換戦略ではなく上位レイヤ戦略。

## 6. 既知の不明点（現時点で公開されていない情報）

- 具体的なデータモデル／ストレージフォーマット（log の物理表現、圧縮、GC、tombstone 回収など）。
- Git ↔ DeltaDB ブリッジの双方向変換の詳細（特に rebase、cherry-pick、shallow clone との整合）。
- ブランチ・マージのモデル（patch theory ライクな順序非依存マージか、Git ライクなブランチ概念を載せるか）。
- スケーラビリティ特性（モノレポ、巨大履歴、多数 replica）。
- アクセス制御・権限モデル（CRDT の特性上、後付けが難しい領域）。
- リリース時期、API 仕様、SDK の有無。

これらは公開ロードマップが出るまで判断保留。

## 7. 関連技術／系譜

- **Operation-based / patch-based VCS**: darcs, Pijul, Fossil。理論的には patch theory／category-theoretic merge。
- **CRDT 系コラボエディタ**: Google Docs, Figma, Zed multiplayer。Yjs / Automerge は CRDT ライブラリの代表。
- **永続的位置参照**: Tree-sitter ベースの semantic anchor、Sourcegraph の SCIP、`git-blame` の line tracking。DeltaDB は CRDT identity を使うため理論的にこれらより堅牢な anchor を提供できる。
- **AI エージェント協働基盤**: OpenAI の "evolving spec" 議論、Anthropic の Computer Use 系、各社の MCP。DeltaDB は「エージェント↔コード↔人間の対話」を VCS 層で受ける狙い。

## 8. 自プロジェクト（yoi）への含意メモ

参考材料として残す（採用の可否ではない）。

- LLM エージェントのインタラクション履歴をコードに永続的に紐付けたい場面（例: 「この修正はどの会話・どのプロンプトから来たか」）で、character-level permalink の発想は流用余地がある。
- ScopedFs を将来スクリプティング言語に公開する計画（memory: project_scopedfs_scripting）と組み合わせる際、エージェントの編集系列を operation log として残すかどうかは設計判断ポイント。
- 短期的には DeltaDB そのものを依存に取り込む選択肢はないが、「commit 未満の粒度をどう持つか」という設計議論の参照点として有用。

## 9. 参考リンク

- [Sequoia Backs Zed's Vision for Collaborative Coding — Zed Blog](https://zed.dev/blog/sequoia-backs-zed) — 一次情報源
- [How CRDTs make multiplayer text editing part of Zed's DNA — Zed Blog](https://zed.dev/blog/crdts) — DeltaDB の基盤となる Zed の CRDT 実装解説
- [Partnering with Zed — Sequoia Capital](https://sequoiacap.com/article/partnering-with-zed-the-ai-powered-code-editor-built-from-scratch/) — 投資側の論点
- [Zed Raises $32M in Series B, Pivots to DeltaDB — Hypeburner](https://hypeburner.com/blog/news/zed-deltadb)
- [Zed Raises $32M ... Unveils DeltaDB — Menlo Times](https://www.menlotimes.com/post/zed-raises-32-million-in-series-b-to-build-next-gen-operation-based-version-control-unveils-deltad)
- [Zed Industries Raises $32M ... DeltaDB — CXO Digital Pulse](https://www.cxodigitalpulse.com/zed-industries-raises-32-million-to-redefine-ai-powered-code-collaboration-with-deltadb/)
- [DeltaDB From Zed — shapeof.com (August Mueller)](https://shapeof.com/archives/2025/8/deltadb_from_zed.html) — 第三者の所感
- [Conflict-free replicated data type — Wikipedia](https://en.wikipedia.org/wiki/Conflict-free_replicated_data_type)

# AGENTS.md の取り込み

## レビュー状態

初回レビュー実施済み。[agents-md-ingestion.review.md](agents-md-ingestion.review.md) を参照。
要件・完了条件すべて達成、無条件で受け入れ可。指摘1（切り詰め時の UTF-8 pop ループの非一貫）は任意修正。

## 背景

[agents.md](https://agents.md) は AI コーディングエージェント向けにプロジェクト固有の指示を置く標準ファイルとして普及しつつある（主要エージェント群が対応し、OpenAI の本体リポジトリだけでも 88 個の AGENTS.md が置かれている）。Insomnia の Pod も、作業ディレクトリに配置された `AGENTS.md` を自動でシステムプロンプトの文脈として取り込めるようにしたい。

agents.md 仕様では「編集対象ファイルから見て最近接の1つ」を読むモデルだが、Insomnia の Pod は cwd が1点に固定されるため、**cwd 直下の `AGENTS.md` のみ**を対象とする。サブプロジェクト向けにカスタム文脈を与えたい場合はそのディレクトリを cwd にして Pod を立てる運用を想定する。

## 依存

- `system-prompt-template.md`（実装済み / `crates/pod/src/system_prompt.rs`）: 本チケットは読み取り結果を `SystemPromptContext.files` マップ経由でテンプレート変数として露出する。`files` キーは既に予約済み。

## spec との整合

`https://agents.md` の仕様は薄く、以下だけを規定している:

- ファイル名は `AGENTS.md`、リポジトリルートに配置
- ネスト時は**最近接優先、マージしない**
- 中身は任意の Markdown、必須セクション無し
- 優先順位は chat prompt > AGENTS.md

サイズ上限・エンコーディング・不在時挙動・テンプレート機能は spec で規定されておらず、各エージェント実装の裁量。本チケットの取り決めは下記の通り spec と矛盾しない。

## 要件

### 探索

- Pod の cwd 直下に `AGENTS.md` があれば読む。親ディレクトリへの遡及は行わない。
- 子ディレクトリのネスト AGENTS.md も扱わない（cwd 直下の1ファイルのみ）。
- ファイルが存在しない場合は欠損ではなく「空」として扱い、エラーにしない。

spec のネスト解決（nearest-wins）は、Pod の cwd が1点に固定される都合で「cwd 直下の1ファイルのみ」に単純化している。サブプロジェクト向けにカスタム文脈を与えたい場合はそのディレクトリを cwd にして Pod を立てる運用。

### テンプレートへの露出

- `SystemPromptContext.files` マップに `agents_md` キーとして本文を挿入する。
- 評価タイミングは system-prompt-template と同じく first turn 開始時の1回のみ。compact 後も再評価しない。
- **不在・読み取り失敗時は `files` マップにキーを入れない**。テンプレ作者は `{% if files.agents_md is defined %}...{% endif %}` で存在をガードする前提（minijinja の `UndefinedBehavior::Strict` の下で安全に扱うため）。

### サイズ上限

- **64KB を上限**とする（約 20-25k token 相当、provider の 30k token/分レートリミットに対して余裕を持った値）。
- 超過時は**先頭 64KB を採用し末尾に `\n\n[truncated: AGENTS.md exceeded 64KB limit]` を追記**。エラーにはしない。
- `tracing::warn!` で切り詰めた旨をログ出力する。

### 読み取り失敗時

- 非 UTF-8、I/O エラーなど `std::fs::read_to_string` が失敗するケースは、**`tracing::warn!` を出して `files` マップにキーを入れずに続行**。Pod 起動は成功扱い。
- ファイル自体が存在しない場合は正常系（warn は出さない）。

### warn の伝達

- 現状の Pod/Worker 層には「ユーザー可視のユーザー通知チャネル」は未整備（`tracing::warn!` のみ）。本チケットでは `tracing` に出すだけとし、TUI バナーや PodEvent への露出は別チケットで扱う。

## 完了条件

- Pod の cwd 直下に `AGENTS.md` を置き、`files.agents_md` を参照するテンプレートの Pod を起動すると、その内容が first turn のシステムプロンプトに反映される。
- `AGENTS.md` を置かない場合でも Pod が正常に起動し、テンプレート側で `{% if files.agents_md is defined %}` ガードを書いていればシステムプロンプトが壊れない。
- 64KB 超の `AGENTS.md` は先頭 64KB + 切り詰め注記でレンダリングされ、`tracing::warn!` がログに出る。
- 非 UTF-8 の `AGENTS.md` があっても Pod 起動は成功し、`files.agents_md` は未定義扱いになり、`tracing::warn!` がログに出る。
- compact を跨いで AGENTS.md の内容が再読込されないことを担保する。

## 範囲外

- 親ディレクトリ方向への遡及、ネスト AGENTS.md のマージ。
- ユーザ単位の共通設定ファイル（将来 Insomnia 独自の user config として別チケット化）。
- AGENTS.md 以外のフォーマット（CLAUDE.md 等）への自動対応。必要なら各自シンボリックリンクで解決する前提。
- ユーザー可視の warn 通知チャネル（TUI バナー等）。本チケットは `tracing::warn!` に出すのみ。

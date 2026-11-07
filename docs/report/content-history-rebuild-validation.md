# 内容・編集履歴基盤の再構築検証

## 対象と境界

T-835 の本文 digest `9a896dbeee3c711895a56de73a3734c5d33967f4ac5688f2ec4f2cb70c09f6ae` を対象とし、指定基点 `d9295f44391a8cde7233231ad156f740e5f20097` から実装した。旧 T-822 の承認や試験結果は新候補の証拠として使わない。

純粋 model は内容と履歴を型付き ID・versioned canonical bytes で区別する。実 SQLite adapter は呼出し元の transaction を借り、認可済みの Workspace／対象から採用済み履歴に到達する読取りだけを許す。設定／Ticket adapter は別 crate の契約例であり、本番の設定採用・policy・移行、CLI、Ticket 編集 API・履歴 UI は変更しない。

永続 adapter の schema は本番 startup に登録していない。Server 接続試験は実 Server store を隔離 DB に作り、その既存 transaction helper 内で schema と契約例を接続する。本番採用側の正規 schema 登録・既存設定移行は T-838 の責務であり、本 fixture の成功で代替しない。認可 fixture は trusted Host の exact-target decision を代替し、HTTP の認証や production policy cutover の試験とは主張しない。

## AC と確認する境界

| AC | 所有する試験・観測 |
| --- | --- |
| 1 内容・構造・意味順・種別・形式 | model の固定 canonical bytes／SHA-256 vector、map 列挙順、sequence 順、厳密 decoder。adapter の設定／Ticket 固定 root |
| 2 旧全文復元・共有・構造差分 | model の copy-on-write と等しい部分木を読まない diff、追加／削除、store 再open と祖先復元 |
| 3 設定／Ticket 共通利用 | adapter の完全 source・明示 content type・directory hierarchy、投稿 ID／順／原著者の往復。Server fixture で別の業務状態を巻き戻さない |
| 4 履歴 CAS／ABA | 実 store の A→B→A、新 parent、新 history identity、2接続で同じ head を観測した編集の stale 拒否 |
| 5 永続 transaction | 実 SQLite の object／history／reachability／head 故障注入、caller rollback、再open。Server の source／projection／history 同 Tx・業務書込み失敗 rollback |
| 6 不完全／改変／互換 | decoder の全 truncate／trailing／非canonical／未知形式／不正 child、stored bytes 改変、immutable save 拒否、固定 v1 bytes の decode／hash 維持 |
| 7 認可／保持／消去 | 他 Workspace／他対象／hashのみ／未採用 object 拒否、handle 取消再検査、共有 dedup、未採用準備の GC、全祖先保持、明示消去と共有対象保持・旧 root 復活拒否 |
| 8 文書・関連・cross-target | crate README／rustdoc に所有境界と互換・消去限界。native と raw WASM の同じ固定 vector、関連 config crate、Server fixture、root check、format／命名／diff |

## 実行記録

初期 checkpoint に対し `CARGO_INCREMENTAL=0 cargo check -j2 -p content-history-store -p content-history-adapters` が成功した。実 store の初回 `transactions` target は 7 件成功。model 作成途中の初回 `contracts` は 11 件成功・固定 fixture 件数不整合で 1 件失敗し、fixture を確定した。失敗を旧 green で置き換えない。

共有編集窓の引渡し後、Server dev-dependency に対応する確定 lock 差分を待つ段階で `cargo test --locked -j2 -p content-history --test contracts` は lock 更新要求により build 前に停止した。T-836 公開 `2998f53de76a6e44e911cf9ba3624ae22c2ae7bc` を直接確認し、Server の依存 3 行のみを採用した。他系列の source や依存は採用せず、lock を並行再生成していない。

最終候補の検証では、Rust コマンドに `CARGO_INCREMENTAL=0` と `-j2`、依存解決に `--locked` を使った。

- `cargo test -p content-history -p content-history-store -p content-history-adapters -p config-source -p config-source-wasm`：model 14、store 10、adapter 12、config-source 54、config-source-wasm 3 件と doctest が成功。
- `cargo test -p yoi-workspace-server --lib content_history_tests`：初回は Ticket fixture 成功、設定 fixture が誤った Decodal コメント `//` により構文検証で失敗。fixture を正しい `#` へ修正した再実行で 2 件成功。Server の既存 dead-code 警告は残る。
- root `cargo check`：TUI／Server を含む default closure 成功。
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps -p content-history -p content-history-adapters -p content-history-store`：成功。
- `cargo fmt --all -- --check`、`git diff --check HEAD`、状態契約命名検査、Deno parity runner の format／type check：成功。既存 config-source の TypeScript export 試験が出力した未整形ファイルはそのファイルだけ Deno format し、基点との差分がないことを確認。

### Native／WASM

native 試験の compiler は Nix Rust 1.95.0（`59807616e`）。最初に公式 1.95.0 WASM std を隔離取得し、公表 SHA-256 `5587b89ff69623d09e476439d44a24453b4e4ea3d5e0b53a5c0a935151ff3fd1` と照合したが、Nix compiler と公式 std の metadata が非互換で E0514 となった。これは hash 不一致の観測ではない。

実 parity には既存の公式 Rust 1.96.0（`ac68faa20`）と対応する公式 WASM std を使い、std と build 出力を `target/t835-wasm/` 配下へ隔離した。配布物の公表 SHA-256 `5a360604f50aaf62320bd2201f6e97113bbdb2a93881c66889dd90cdbaab51a0` との一致を確認した。既存 toolchain の host linker wrapper が消失済 Nix path を参照していたため最初の host build-script link は失敗した。global toolchain は変更せず、検証専用 wrapper で host link のみ BFD を指定し、WASM link は既存 LLD を指定した。

`cargo build -p content-history --example wasm_parity --target wasm32-unknown-unknown` 後、`deno run --allow-read crates/content-history/tests/wasm_parity.ts <wasm>` を実行し、host import のない実 WASM module で **9 個の固定 object の全 digest word が native と同じ fixture に一致**した。build だけを parity 成功としていない。稼働 Server／Runtime、実 DB、global toolchain は更新していない。

## 未確認範囲

本番設定採用・移行、実 Ticket 編集 API、履歴 UI、HTTP 認証経路、稼働環境への適用、任意 OS crash／secure disk overwrite／backup 消去は本件の試験結果で証明しない。通常 Rust target は通常 Cargo runner の選択対象となる。raw WASM parity は明示的な cross-target 実行であり、CI での実行を確認したという記録ではない。

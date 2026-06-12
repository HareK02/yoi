# テスト妥当性レビュー: project-record
- 評価: 概ね良い

## 確認範囲

- Workspace root: `/home/hare/Projects/yoi`
- Crate: `crates/project-record`
- Main source/test file: `crates/project-record/src/lib.rs`
- Crate の責務:
  - Unix epoch milliseconds を、path-safe な固定幅13文字の Crockford base32 風 ID に encode/decode する。
  - ID の validation を提供する。
  - collision 時に suffix を付けず、millisecond 値を bounded に increment して未使用 ID を割り当てる。
  - Ticket / Objective などの project record ID 生成・検証の共通基盤になっている。

## 現在のテストがよくカバーしていること

- `encode_unix_epoch_millis` の基本境界を押さえている。
  - `0 -> 0000000000000`
  - `31 -> 000000000000Z`
  - `32 -> 0000000000010`
- 固定幅 ID の lexicographic order が numeric / chronological order と一致する、というこの crate の重要 invariant を明示的にテストしている。
- ambiguous / path-unsafe な文字を reject するテストがある。
  - `I`, `L`, `O`, `/` を拒否しており、path-safe ID としての意図は確認できる。
- decode overflow 相当のケースも一応含まれている。
  - `ZZZZZZZZZZZZZ` は文字種としては valid だが `u64` を超えるため invalid になる。
- collision allocation の基本動作を押さえている。
  - 既存 ID と衝突したら `base_millis + 1` を使う。
  - 全候補が衝突する場合は `MAX_COLLISION_PROBES` で bounded に失敗する。

## 不足 / 疑問のあるテスト

- encode/decode の round-trip coverage が薄い。
  - `decode_unix_epoch_millis("0000000000010") == 32` 以外は decode の成功系がほぼ確認されていない。
  - `u64::MAX`、現在時刻に近い値、桁境界、複数の代表値で `decode(encode(x)) == x` を見ると、crate の中心 invariant がより強くなる。
- `decode_digit` の valid alphabet coverage が不足している。
  - 現在の成功系 decode は主に `0`, `1` 周辺に偏っている。
  - `A-H`, `J-K`, `M-N`, `P-T`, `V-Z` の range 分岐が壊れても一部は検出しにくい。
- overflow 境界が明確にテストされていない。
  - `decode("FZZZZZZZZZZZZ") == u64::MAX`
  - `decode("G000000000000")` や `decode("ZZZZZZZZZZZZZ")` が `TimestampOverflow`
  のように、13桁 base32 が 65bit 空間であることに由来する境界を分けて確認したい。
- `rejects_ambiguous_or_path_unsafe_characters` は少し名前と中身が混ざっている。
  - `ZZZZZZZZZZZZZ` は ambiguous/path-unsafe character ではなく numeric overflow なので、invalid character 系と overflow 系は別テストにした方が失敗時の意味が明確になる。
- `allocate_record_id` の overflow 経路が未テスト。
  - `base_millis == u64::MAX` かつ最初の候補が衝突した場合、次の probe で `TimestampOverflow` になるはず。
  - collision boundedness だけでなく、上限付近の arithmetic safety を確認したい。
- lowercase policy がテストされていない。
  - Crockford base32 という名前からは lowercase 許容を期待する読者もいるため、この実装が uppercase-only なら `a`, `z` などを reject するテストか、仕様コメントでの明示があると良い。
- `unix_epoch_millis_now` は実質未テスト。
  - `SystemTime::now()` wrapper なので優先度は低いが、少なくとも「現在の正常環境で `Ok` を返す」程度の smoke test は可能。
  - ただし時刻依存なので、無理に強い assertion を置く必要はない。

## 追加を提案するテスト

- 代表値での round-trip test:
  - `0`
  - `1`
  - `31`
  - `32`
  - `33`
  - `1024`
  - 現在時刻近辺の固定値
  - `u64::MAX`
- decode boundary tests:
  - `FZZZZZZZZZZZZ` が `u64::MAX` に decode されること。
  - `G000000000000` や `ZZZZZZZZZZZZZ` が `TimestampOverflow` になること。
- alphabet mapping test:
  - `RECORD_ID_ALPHABET` の各文字を下位桁に置いた ID を decode し、index と一致すること。
- invalid character test の分割:
  - ambiguous chars: `I`, `L`, `O`
  - lowercase chars: `a`, `z` など、uppercase-only policy を明示する場合
  - path separators / unsafe chars: `/`, `\`, `.`, maybe whitespace
  - overflow: valid alphabet だが `u64` 範囲外
- allocation overflow test:
  - `allocate_record_id(u64::MAX, |_| true)` が `TimestampOverflow` になること。
- allocation probe behavior test:
  - `exists` が必要以上に呼ばれないこと、また最初の available candidate で止まることを確認すると、bounded allocation の仕様がより明確になる。

## 実行したコマンド

- `cargo test -p project-record`
  - 結果: pass
  - 概要: 5 unit tests passed, 0 failed; doc-tests 0 passed / 0 failed.
  - 注記: artifact directory の file lock 待ちがあったが、テスト妥当性評価に影響する失敗ではない。


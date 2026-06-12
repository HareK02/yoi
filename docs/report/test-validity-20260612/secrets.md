# テスト妥当性レビュー: secrets
- 判定: 概ね良い

## 確認範囲
- 対象 crate: `crates/secrets`
- 読んだファイル:
  - `crates/secrets/Cargo.toml`
  - `crates/secrets/README.md`
  - `crates/secrets/src/lib.rs`
- 既存テスト: `crates/secrets/src/lib.rs` 内の unit tests 5件
- 変更: なし

`secrets` crate の責務は、provider-independent な secret id/value のローカル保存、ID 検証、軽量な plaintext-at-rest 低減、改ざん検出、store file format の読み書きに絞られている。

## 現在のテストがよくカバーしていること
- 基本 lifecycle は押さえられている。
  - `set` / `get` / `list_ids` / `delete` / 削除後 `NotFound` を `roundtrip_list_delete` が確認している。
- ID validation の主要な危険入力は確認されている。
  - empty、absolute path、`..` traversal、double slash、空白、改行、`~`、長すぎる ID、代表的な valid ID を検証している。
- fail-closed 系の重要性は理解されたテストになっている。
  - ciphertext 改ざん時に `Decode` になる。
  - wrong key で復号できない。
- secret value の露出防止に関する最低限の invariant はある。
  - `SecretValue` の `Debug` が plaintext を含まない。
  - store file に plaintext が直接書かれない。
- テストは tempdir と固定 key を使っており、外部環境や実 user secret store に依存しない点は良い。

## 不足 / 疑問のあるテスト
- file format error path の coverage が薄い。
  - invalid JSON は `Error::Parse` になるか。
  - unsupported `version` は `Error::UnsupportedVersion` になるか。
  - entry の `nonce` / `ciphertext` / `tag` が invalid hex、odd length、欠落、型不一致の場合に fail-closed するか。
- ID validation の境界値が少し不足している。
  - `MAX_ID_LEN` ちょうどの ID が通ること。
  - `.`、`x/.`、`x/`、`a//b`、non-ASCII、colon/backslash などの representative invalid cases。
  - 可能なら error variant まで見ると、将来の validation 退行を発見しやすい。
- encryption/encoding の境界値が不足している。
  - 空文字 secret。
  - 32 bytes を超える multi-block secret。
  - non-ASCII UTF-8 secret。
  - 同じ id/value を overwrite したときも復号できること。
  - nonce/ciphertext が毎回同一にならないことは、この実装の情報漏洩低減 invariant として確認する価値がある。
- store 操作の細かい API invariant が未確認。
  - empty store の `list_ids()` が empty を返す。
  - missing id の `delete()` が `Ok(false)` を返し、既存 store を壊さない。
  - existing id の overwrite が正しく最新値を返す。
  - `SecretStore::new(data_dir)` が `data_dir/secrets/store.json` を使い、同じ `data_dir` で reopen 可能であること。
- atomic write 周辺のテストは実質ない。
  - parent directory が自動作成されることは roundtrip で間接的に通っているが、明示されていない。
  - successful save 後に tmp file が残らないこと。
  - write/rename failure の portability ある検証は難しいが、この crate が file persistence を所有するなら少なくとも successful path の file-state invariant は追加できる。
- `SecretStore` の `Debug` は derived で `key` を出す可能性がある。
  - plaintext secret ではないが、local obfuscation key material を diagnostics に出す設計でよいかは確認対象。
  - 少なくとも「SecretValue だけが redacted 対象」という意図なら明文化、そうでないなら redaction 実装とテストが必要。
- 現在の corruption tests は private `StoreFile` を直接使う unit test で、file format owned crate としては妥当。
  - ただし JSON wire format の互換性を守りたいなら、private struct 経由だけでなく literal JSON fixture を使うテストもあると version/schema 退行に強くなる。

## 追加を提案するテスト
- `loads_empty_missing_store`:
  - new temp path で `list_ids()` が `[]`。
  - missing id `get` は `NotFound`。
  - missing id `delete` は `false`。
- `id_validation_boundaries`:
  - `MAX_ID_LEN` exactly accepted。
  - `MAX_ID_LEN + 1` rejected。
  - `.` / `x/.` / `x/` / `x\\y` / `x:y` / `日本語` rejected。
- `store_file_errors_fail_closed`:
  - invalid JSON -> `Parse`。
  - `version: 999` -> `UnsupportedVersion`。
  - invalid hex in each entry field -> `Decode`。
- `roundtrip_value_boundaries`:
  - empty string。
  - long value over multiple SHA blocks。
  - UTF-8 value such as `日本語-secret-🔑`。
- `overwrite_preserves_latest_value`:
  - same id を異なる value で set し、latest のみ返る。
  - `list_ids` に duplicate が出ない。
- `same_plaintext_reencrypts_with_new_nonce`:
  - same id/same value を二度 set し、保存 JSON の nonce または ciphertext が変わることを確認する。
- `new_uses_data_dir_store_path_and_key`:
  - `SecretStore::new(dir)` で保存し、同じ `dir` の store で読める。
  - 別 `dir` の derived key では decode できない。
- `successful_save_file_state`:
  - parent directory 作成。
  - `store.json` が valid JSON。
  - success 後に sibling tmp file が残らない。

## 実行したコマンド
- `cargo test -p secrets`

結果:

- unit tests: 5 passed
- doc-tests: 0 passed
- failures: none


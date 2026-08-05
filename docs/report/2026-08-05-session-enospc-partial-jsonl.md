# ENOSPC で Session JSONL の末尾が UTF-8 途中切れになる

## 観測

`Companion1` (`worker-runtime-30`) の直近 Segment
`019fce34-eb04-7420-9cc5-0e7e12f5bd67` は、Workdir の Edit tool が
`no storage space` で失敗した後、会話ログ `.jsonl` の末尾が `0xe3` 1 byte
だけで終わっていた。これは 3 byte UTF-8 文字の先頭 byte であり、実際の末尾は
「Workdir のストレー」の次の文字の途中で切れていた。

同 Segment の `.trace.jsonl` は valid UTF-8 だった。再開時の
`stream did not contain valid UTF-8` は provider stream ではなく、Session log を
`fs::read_to_string` した際のエラーだった。

## 原因

`FsStore::append_line` は JSON 本文と改行を別々に `write_all` し、ENOSPC で
部分書き込みになっても元の file length へ戻していなかった。reader も file 全体を
UTF-8 String として読むため、newline に到達していない未コミット末尾だけで Segment
全体を復元不能にしていた。

さらに Engine の history append callback と SystemItem committer は、Store error を
warning にして drop していた。このため disk 上の history を更新できなくても memory
上の history と tool loop が先へ進み得た。

## 改善

- newline を JSONL record の commit marker とする。
- reader は newline 未到達の末尾を未コミット record として無視する。
- 次回 append 前に未コミット末尾を最後の newline まで truncate する。
- append の partial write は append 開始時の file length へ rollback する。
- repair/write/rollback は `FsStore` clone 間で直列化する。
- Engine history append を fallible にし、Store write 成功前には item を memory history
  に入れない。
- tool call の永続化に失敗した turn は tool 実行前に停止する。
- SystemItem の commit failure も transient context injection にせず turn error にする。
- `Invoke` 後に terminal run record が無い Segment は restore 時に interrupted とする。
  これにより crash/ENOSPC 後の dangling tool call は新しい user turn の前に閉じられ、
  side effect を無条件に再実行しない。

## 境界

この修正は process interruption と ENOSPC による trailing partial record を対象にする。
newline 済み record 内部の破損は silent recovery せず `StoreError::Corrupt` のまま扱う。
また append ごとの `fsync` は追加していないため、突然の電源断に対する block-level
durability まで保証するものではない。

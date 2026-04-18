# Scope lock file: write 排他とスコープ分譲の記録基盤

## 背景

Pod オーケストレーションでは scope の分譲（spawner が自身の scope を spawned Pod に譲渡）が発生する。また、人間が独立に複数の Pod を起動した場合にも同一パスへの write 衝突を検出する必要がある。

これらを解決するため、マシン上の全 Pod の scope 割り当てを**単一の lock file**で一元管理する。

## 仕様

### lock file

置き場: `$XDG_RUNTIME_DIR/insomnia/scope.lock`

内容:
```json
{
  "allocations": [
    {
      "name": "abc123",
      "pid": 12345,
      "socket": "/run/insomnia/.../pod-a.sock",
      "scope_allow": ["/project/src:write:recursive"],
      "delegated_from": null
    },
    {
      "name": "def456",
      "pid": 12346,
      "socket": "/run/insomnia/.../pod-b.sock",
      "scope_allow": ["/project/src/core:write:recursive"],
      "delegated_from": "abc123"
    }
  ]
}
```

アクセスは `flock(2)` による advisory lock で排他制御する。

### 操作

| タイミング | 動作 |
|---|---|
| **Pod 起動** | lock → stale 検出（PID 死活）→ 自動回収 → write 衝突チェック → 自分の scope を登録 → unlock |
| **scope 分譲** | lock → spawner の allocation に deny 追記 → 新 Pod の allocation を追加（`delegated_from` に spawner）→ unlock |
| **Pod 正常終了** | lock → 自分の allocation を削除 → `delegated_from` が自分の子が残っていなければ親の deny を解除 → unlock |
| **stale 検出** | `kill(pid, 0)` で生存確認。死んでいたら allocation を削除し scope を `delegated_from` の親に返却 |

### stale の自動回収

Pod がクラッシュした場合、lock file にエントリが残る。次に lock file を開いた Pod が stale を検出し自動回収する:

- 死亡 Pod の scope のうち、生存中の子 Pod が持つ分を除外
- 残りを `delegated_from` の親に返却
- 死亡 Pod のエントリを削除
- 子 Pod の `delegated_from` を親に付け替え

### effective scope の導出

```
effective_scope = 自分の allocation - Σ(delegated_from が自分を指す子の allocation)
```

### セキュリティとアクセス

- ファイルパーミッション `0600`（owner only）、ディレクトリは `0700`。他ユーザーからの読み取りを防ぐ
- owner（Pod を動かしているユーザー）は当然読める。JSON なので直接確認も可能
- Pod による lock file 探索は排他制御の目的に限定する。Pod 発見のための lock file スキャンは行わない（Pod の発見は spawn 記録 + 明示的な紹介のみ）
- 衝突で Pod 起動が拒否されたとき、競合相手の name をエラーメッセージに含める

## 実装

- 新規モジュール `crates/pod/src/scope_lock.rs`（または `crates/scope-lock/`）
- Pod 起動時（`Pod::from_manifest` / `Pod::from_manifest_toml`）に lock 取得
- Pod 終了時（`Drop` または明示的 release）に lock 解放
- Controller 層でのエラー伝搬

## 完了条件

- Pod 起動時に scope lock file に allocation が記録される
- 同一パスへの write 衝突が検出され、Pod 起動が拒否される（競合相手の name がエラーに含まれる）
- Pod 正常終了時に allocation が削除される
- stale エントリ（PID 死亡）が自動回収され、scope が親に戻る
- 分譲チェーン（A→B→D）の部分回収が正しく動作する
- 単体テストで衝突検出・stale 回収・分譲/返却が検証される

## 範囲外

- SpawnPod ツール自体の実装（`tickets/spawn-pod-tool.md`）
- scope の分譲粒度（permission レベルでの分譲等）は当面パス単位のみ

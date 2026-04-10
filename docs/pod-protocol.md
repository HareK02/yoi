# Pod Protocol

Pod の制御・監視に使う JSONL ベースのメッセージプロトコル。トランスポートに依存しない。

```
CLI        → Pod Protocol (直接呼び出し)
Native App → Pod Protocol (直接呼び出し)
Web        → 中央バックエンド → daemon (Unix socket) → Pod Protocol
```

## 設計判断

- **リクエストとレスポンスの紐付けはしない**: Pod は1つであり、Pod の状態遷移（イベント）を見れば何が起きているか分かる
- **イベントは全リスナーに broadcast**: 読み取り専用の監視も、操作側も同じストリームを受け取る
- **操作の競合は先勝ち**: run 中に別の run が来たらエラーイベントを返す

## daemon

daemon は Pod Protocol を Unix domain socket 上で中継する薄い層。接続=リスナー登録、切断=リスナー解除。それだけ。

# Spawned Pod 完了通知が来ない経路の疑い

## 観測

Spawned Pod が完了しているにもかかわらず、parent 側に完了通知が来ないことがある。`ReadPodOutput` の assistant text 抽出バグは修正済みだが、完了通知そのものの `PodEvent` delivery path に別の構造的リスクが見つかった。

## 現行の通知経路

child の parent-originated turn が `Finished` になると、child controller の `drive_turn` が parent socket へ `PodEvent::TurnEnded` を fire-and-forget する。

parent 側は `Method::PodEvent` を受けると side effect を適用し、`NotifyBuffer` に typed event を積む。parent が idle なら `RunForNotification(PodEvent)` が auto-kick され、interceptor が `SystemItem::PodEvent` として history に commit し、LLM request に通知が乗る。

## 疑い

送信 helper `connect_and_send` は socket に接続して Method を 1 行 write し、応答を読まずに close する。一方、受信側 `SocketServer::handle_connection` は Method を読む前に alert snapshot と `Event::Snapshot` を client に write する。

この組み合わせでは、send-only client が読まない / read half を保持しないため、server 側が snapshot write で失敗または詰まり、Method を読む前に connection handler が終わる可能性がある。これが起きると child の `PodEvent::TurnEnded` は parent controller に到達せず、NotifyBuffer にも入らない。

影響は `PodEvent` だけでなく、`StopPod` が child に送る `Method::Shutdown` など `connect_and_send` 利用箇所全般に及ぶ可能性がある。

## 対応

`tickets/pod-event-callback-delivery.md` を作成した。callback / fire-and-forget Method delivery を server の initial snapshot write に阻害されない形へ修正し、大きな snapshot を持つ parent に対しても `PodEvent::TurnEnded` が届く regression test を追加する。

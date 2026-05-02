# TUI で auto-kick 由来のターンが表示されない

## 背景

Pod が `Method::PodEvent::TurnEnded` などを socket 経由で受信すると、controller は notification を notify buffer に積み、Idle なら `pod.run_for_notification()` で新しいターンを起動する（`crates/pod/src/controller.rs:611-687`）。このターンの assistant 出力 (`Event::TurnStart` / `TextDelta` / `TurnEnd` 等) は通常通り broadcast Event として全クライアント（TUI 含む）に配信されるはず。

## 問題

socat で稼働中の codex-oauth pod の socket に `Method::PodEvent::TurnEnded` を 1 行流したところ、socat 側の subscribe には turn が完全に流れてきた（thinking_delta / text_done / turn_end 取得済み）が、同じ pod を起動している TUI 画面には新ターンが描画されなかった。

`Method::Run` 経由の通常ターンは TUI に表示されるので、broadcast 配信そのものは生きている。auto-kick 由来のターン（user_message を伴わない turn）に固有の表示パスで落ちている可能性が高い。

## 要件

- auto-kick で起動したターン（user 入力を伴わないターン）も、user 由来ターンと同様に TUI 履歴に表示される。
- turn header 等の見た目で「通知由来である」ことを示す表記を入れるかは別議論。

## 完了条件

- 親 pod が PodEvent を受信して auto-kick した際、TUI 上で thinking / assistant text / turn_end が user 由来ターンと同様に表示される。

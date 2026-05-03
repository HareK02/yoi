# TUI で Pod への外部入力 (Notify / PodEvent) が描画されない

## 背景

Pod が `Method::Notify` / `Method::PodEvent` を socket 経由で受信すると、controller は内容を notify buffer に積み、Idle なら `pod.run_for_notification()` で新しいターンを起動する（`crates/pod/src/controller.rs`）。auto-kick されたターンの assistant 出力 (`Event::TurnStart` / `TextDelta` / `TurnEnd` 等) は通常通り broadcast Event として全クライアント（TUI 含む）に配信される。

## 問題

socat で稼働中の codex-oauth pod の socket に `Method::PodEvent::TurnEnded` を 1 行流したところ、socat 側の subscribe には turn が完全に流れてきた（thinking_delta / text_done / turn_end 取得済み）が、同じ pod を起動している TUI 画面には新ターンが描画されなかった。

`Method::Run` 経由の通常ターンは TUI に表示されるので、broadcast 配信そのものは生きている。原因は **Pod が受信した外部入力 (`Method::Notify` / `Method::PodEvent`) が broadcast event として echo されておらず**、TUI からは「何も入力がない状態で突然ターンが始まる」ように見えていることにある。auto-kick ターンが描画されない件はこの下流症状の一つ。

## 要件

- Pod が socket で受信した外部入力のうち、活動ログとして残すべきもの (`Method::Notify` / `Method::PodEvent`) を broadcast event として全 subscriber に echo する。
- TUI はその event を user message / assistant text と並列のログ要素として描画する。
- auto-kick 由来ターン (`TurnStart` 以降) は既存経路で従来通り表示される。Notify / PodEvent 受信が表示されるようになれば、ターン境界の出所はログ上で自然に区別できる。

## 範囲外 / 非目標

- LLM 注入テキスト (`notify_wrapper` 適用後の wrapped string) を UI に見せるかは別判断。本チケットでは **raw メッセージをそのまま echo** する形で着地する。UI 側で wrapper を適用したくなったら、別途 catalog を引く形で対応する。
- `starts_turn` 等の「auto-kick フラグ」を新 event に持たせない。ターン境界制御は `TurnStart` の責務であり、入力 echo event はあくまで入力ログ要素のみを表す。
- protocol に追加する Event variant は **入力 echo の責務だけ**を持ち、UI 通知 (toast / OS 通知) を兼ねない。

## 完了条件

- socket に `Method::Notify { message }` を流すと、全 subscriber（TUI 含む）のログにその通知本文が user / assistant と並列の要素として表示される。
- socket に `Method::PodEvent::TurnEnded` 等を流すと、その受信を示すログ要素 + 後続 auto-kick ターンの thinking / assistant text / turn_end が、user 由来ターンと同様に TUI に表示される。
- 追加した broadcast event は `Method::Notify` / `Method::PodEvent` の payload と一対一対応し、`starts_turn` のような派生フラグを持たない。

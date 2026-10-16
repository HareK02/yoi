# Hosting UIのブラウザ検証

production UIと合成API応答の組合せを確認する。Setupだけはbrowser API境界も`setup-projection.ts`で置換する。Backend認可・実token・暗号署名・native WebAuthnを証明する検証ではない。実sessionやsecretは持ち込まず、request headers/bodyも記録しない。

共有buildは読取り専用とし、画像・診断・一時領域は`target/web-ux`へ分離する。実行入口と境界の理由は各test/capture sourceに置く。自動検査の成功は、画像とreview contextの目視判定を代替しない。

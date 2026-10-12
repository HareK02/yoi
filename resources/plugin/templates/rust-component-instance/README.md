# Rust Component Instance Plugin雛形

提案段階の`yoi:plugin/instance@1.0.0`を対象にしたオフラインpackageの雛形である。YoiはPluginのインストールやWorker内での実行を提供していない。

Tool・Serviceの要求と応答は将来の実行境界を検討するための例である。host APIやgrantのmetadataは現在の製品では権限を付与せず、オフライン検査の成功も実行の許可を意味しない。

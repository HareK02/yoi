# Rust Component Plugin雛形

提案段階の`yoi:plugin/tool@1.0.0`を対象にしたオフラインpackageの雛形である。YoiはPluginのインストールやWorker内での実行を提供していない。

guest側の型付き入出力とエラー表現はRust PDKが担当し、ファイル・ネットワーク・環境への権限は付与しない。`plugin.component.wasm`はpackage検査用のテキストplaceholderであり、実行可能なcomponentの証拠にはならない。

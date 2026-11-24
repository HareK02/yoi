# WorkdirコマンドOOMの隔離調査（2026-11-24）

## 観測と実装上の境界

T-879の調査では、Runtime管理WorkdirとExternal共有CLIが共通のLocal providerへコマンド実行を委譲していた。既存のprocess group回収だけでは、メモリ消費を制限できず、別sessionへ移った子孫の回収も保証できなかった。

Arcadiaではuser systemd managerとcgroup v2のmemory controllerを利用できた。親のtmux scopeや既存のpolicyを変更せず、新しい32 MiB上限・swap無効のscopeだけを作成した。子内で有限上限を読み取り確認してから上限を超える割当を行い、失敗したscopeの`Result=oom-kill`、制御シェルの継続応答を観測した。最初のPythonによるプローブは実行ファイル不在で未実行だった。これはglobal OOMの再現ではない。

検証時の共通祖先`user@1000.service`は`OOMPolicy=continue`だった。この観測は、事故当時の消失済みscopeや別ホストの設定を証明しない。本番Runtime binaryと共有CLI全体を起動した受入試験、後述の運用配置の適用・再生成確認は、この初期調査では行っていない。最終candidateの検証と独立レビューはMRの証拠で判断する。

実装はコマンドのscopeを`app.slice`へ置き、tmuxのscopeの下へ作らない。

```text
user@UID.service
├─ session.slice / tmux-spawn-….scope   # Runtime、共有CLI、制御シェル
└─ app.slice / yoi-command-….scope      # コマンドと子孫
```

同じuser manager配下でもRuntime自身が別のsliceに置かれる構成はあり得る。重要なのはコマンドがRuntimeのunitの子ではないことと、共通祖先でOOMがunit全体の停止へ変換されないことの両方である。共通の`user@UID.service`が`OOMPolicy=stop`等の場合や、祖先の`memory.oom.group=1`を検出した場合は、要求された隔離のまま実行できないため起動を拒否する。ホストのpolicyは書き換えない。

## 運用側で判断する配置

tmux paneの`OOMPolicy=continue`は、コマンドのメモリ制限とは独立した対策である。運用担当者はtmuxが生成するscopeの設定元を確認し、再生成時にも同じpolicyを与える必要がある。既存の一つのscopeに対する一時的な`set-property`だけでは、次のpaneを保護したことにならない。

systemdのdash-prefix drop-inを使う場合の配置候補は、利用者のsystemd設定ディレクトリの`tmux-spawn-.scope.d/50-oom-policy.conf`である。

```ini
[Scope]
OOMPolicy=continue
```

これは本調査で適用・再生成確認済みの設定ではない。運用担当者はuser managerへreloadした後、別の安全なpaneで実際に新規生成された`tmux-spawn-….scope`を特定して、以下の値を確認する。生成元が異なる名前や明示的なtransient propertyを使う場合は、その生成元でpolicyを指定する。drop-inの存在だけで適用済みと扱わない。

```sh
systemctl --user show "$new_scope" -p OOMPolicy -p KillMode -p ControlGroup -p MemoryMax
systemctl show "user@$(id -u).service" -p OOMPolicy -p ControlGroup -p MemoryMax
```

共通の`user@UID.service`のpolicyが不適切な場合は、system managerを管理する運用担当者が、その利用者のunitに対する`user@UID.service.d/`の`[Service] OOMPolicy=continue`を検討する。全利用者へ一律適用する変更とは区別する。reloadや再起動を要する適用は別の保守判断であり、稼働中の自分のRuntimeやpaneをこの検証のために停止しない。

### 所有するtmux生成元での隔離確認

独立レビュー後の追加調査では、共有tmux設定ではなく、専用socketと専用設定のtmuxを起動した。専用の`default-command`がpaneごとに一意の`tmux-spawn-yoi-t879-….scope`を生成し、その生成要求に`OOMPolicy=continue`を指定した。既存scopeへの`set-property`は使わず、初期paneに続く`new-window`でも同じ生成元を通した。両paneの実所属は`user@1000.service/session.slice/…`、実propertyは`OOMPolicy=continue`・`KillMode=control-group`だった。最初のpaneから`app.slice/yoi-command-t879-….scope`へ別の32 MiBコマンドを起動し、その`Result=oom-kill`後にも両paneのシェルが応答した。pane側も有限上限と自動終了期限を持ち、専用tmux・全owned unit・一時rootの回収を確認した。

この確認は、生成元へ明示的なtransient propertyを指定する上記方針に対応する。dash-prefix drop-inの適用やreload・稼働中のtmux生成元の変更は行っておらず、その経路や実運用deploymentを検証したものではない。検証時の専用設定、スクリプト、実階層・property・終了・回収の証拠はMRのR3検証記録で特定する。運用側は自分の生成元がdrop-inと明示propertyのどちらを使うかを区別して判断する。

`OOMPolicy=continue`は、kernelが選んだプロセスを復活させない。メモリ枯渇そのもの、Runtime自身のkernel OOM kill、上位の有限予算によるOOM、systemd-oomdの独立した選択を防ぐ保証でもない。コマンドの有限上限と制御プロセスの余裕を別に確保する。

## Hostの予算と失敗時の扱い

メモリ量はServerやモデルのコマンド引数ではなく、実行するホストの起動境界で決める。RuntimeとExternal共有CLIは別プロセスの予算を持つ。ホスト上の合計圧力は、各providerの`メモリ上限 × 同時実行数`の総和に、Runtime・共有CLI・他の作業の余裕を加えて判断する。各provider内の予約だけをホスト全体のadmissionと誤認しない。

起動環境での明示的な例は次のとおり。8 GiBは調査時の初回ビルド枠に使った例であり、任意のRustビルドやホストに推奨する容量ではない。

```sh
YOI_WORKDIR_COMMAND_ISOLATION=systemd \
YOI_WORKDIR_COMMAND_MEMORY_MAX=8589934592 \
YOI_WORKDIR_COMMAND_PARALLEL=1 \
  <Runtimeまたは共有CLIの通常の起動コマンド>
```

実行可否の確認では、user managerへの接続、systemdのscope property対応、cgroup v2のmemory controller、共通祖先のpolicyを調べる。scope内の固定wrapperは実際の所属と上限・swap・OOM groupを確認するまで利用者のコマンドを起動しない。必要な機能や権限がなければunconfinedへfallbackしない。可搬性のため既定は隔離無効であり、その場合は有限メモリ境界を提供したとは表示しない。

単なるexit 137やSIGKILLはOOMの証拠ではない。実行ごとの失敗scopeから得た`Result=oom-kill`だけをsystemd OOMとして帰属し、cancel・timeout・IO障害・出力回収の不完全さとは別に保持する。managerの観測や停止確認に失敗したときは不明のまま返す。停止が不明な実行のメモリ予約は解放せず、新たな実行による予算超過を抑える。運用担当者は残存unitと実プロセスを確認して原因を解消し、必要なprovider再起動を別途判断する。失敗・応答喪失したコマンド自体を自動再実行しない。

## 検証の選択

通常suiteと、systemd/cgroupが必要な明示suiteを分ける。`deno task test:workdir:systemd`は、専用の有限上限を持つコマンドだけでOOM、別実行の生存、孫の回収、後続コマンド、HTTPからBashへの結果伝播を検証する入口である。build自体の予算や同時実行枠は別途確保する。この明示suiteの追加をCI実行済みの証拠にはしない。本調査時点のcheckoutには対応するCI jobの配線はなく、実行ホスト・権限を持つjobの整備は未確認である。

HTTP/Bashのcomponent試験は本番のclient・dispatcher・provider・collectorを接続するが、HTTP session openingはfixtureである。Runtimeの署名認証、Externalのgrant作成・WebSocket登録、実際のtmux pane再生成を含む試験の代わりにはしない。

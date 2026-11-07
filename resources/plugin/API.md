# Workspace Plugin APIの設計契約

本書はWorkspace管理のPluginに採用する設計を定める。実行形式はWebAssembly Component Model、engineはWasmtime、guest/Host境界はWITとする。実装済みAPIの利用ガイドではない。旧[v1 Tool world](legacy/wit/yoi-plugin-tool-v1.wit)、[v1 instance world](legacy/wit/yoi-plugin-instance-v1.wit)、[v1 Host imports](legacy/wit/deps/yoi-host/yoi-host-v1.wit)は歴史資料として隔離する。v2の機械的契約は[Tool worlds](wit/yoi-plugin-v2.wit)と[Host imports](wit/deps/yoi-host/yoi-host-v2.wit)で定める。PDKやtemplateのbuild成功はHost実装やWorker実行経路の完成を意味しない。

管理、guest実行、Server上の拡張データを横断する判断をWIT資産の近くで一か所に保持する。実装時には各wire型と境界testを契約の機械的な根拠にし、ここには所有境界・失敗保証・採用理由を残す。作業順と進捗の正本はO-4、T-469、T-465、T-533とする。

## 1. 三つのAPIと所有境界

Pluginを管理する権限とPluginコードに渡す権限を分離する。Serverに保存するからServer operatorしか導入できない、とはしない。

| 境界 | 呼び出し方向 | 責任 |
| --- | --- | --- |
| Workspace管理API | 認可された管理client → Server | 導入・更新・失効・削除、設定、検証結果、診断 |
| Component exports | Runtime Host → guest | Tool呼び出し、Service/event処理 |
| Host imports | guest → Runtime Host → 必要なdomain owner | 外部通信、拡張データ、診断等の限定された作用 |

ServerはWorkspace所属、設定、installation、artifact、永続操作の正本を持つ。Runtimeはplanを適用し、guestと接続・一時handleを所有する。公開HTTP DTOは`server-api`、domain認可と永続化は`workspace-server`、guest ABIは`resources/plugin/wit`とPDKの責任とする。Rust traitやServer RESTを汎用proxyとしてguestへ渡さない。

Workspace管理者は自Workspaceに導入・更新できる。読取りcallerにはWorkspace読取り権限、mutationにはWorkspace管理権限を要求し、既存の認証済みactorから判断する。operator権限は通常導入の前提にしない。Worker/model/repository/packageの自己申告は管理権限にならない。

Hostが実行する全操作の上限は、packageの要求、Workspaceの付与、Profileの縮小、呼出元から委譲可能な範囲、現在の対象資源・運用制約の積集合とする。Serviceはlaunch時に明示されたService用grantを使い、管理者のsession credentialを借用しない。SubWorkerへのPlugin継承も再解決された委譲範囲内のみとし、親のhandleを移送しない。

## 2. identityと保存するもの

installationはWorkspaceに属し、artifactはimmutable bytesで識別する。同じpackage名やversionは権限でも実体の一意性の証拠でもない。

- `installation_id`はServerが発行するWorkspace内の導入identity。package作者の`id`は名前であり、このidentityを置換しない。
- 一つのinstallationに複数のimmutable artifactを登録できる。updateは新しいdigestの登録であり既存bytesを置換しない。同じinstallation内のpackage id/versionに異なる内容を登録する場合はversion衝突として拒否する。他Workspaceの同名versionは検査・存在開示の対象にしない。
- 同一package idの別installationは許すが、設定aliasを一意にする。同じpackageを別credentialで使う用途をversion名の改変で代替しない。
- Tool公開名はinstallationの不変ID由来namespaceと宣言Tool名からHostが構成する。表示aliasの変更で既存planのTool名を変えない。衝突・長さ超過はplan検証時に拒否し、登録時に黙ってrenameしない。
- artifact blobの共有は物理dedupeに限定する。他Workspaceのcatalog、設定、導入履歴、data、grantは共有されない。hashを知っているだけでは取得できない。

### package digestと配送digest

既存`manifest::plugin`のdigestは、path順のfile集合をhashするpackage identityであり、ZIP全体のSHA-256ではない。両者を同じfieldとして検証しない。

- `package_digest`は既存のfile集合digestを維持する。schema/manifest/componentを含む論理内容の同一性を検証する。
- `artifact_digest`はServerが保存・配送するcanonical archive bytesのSHA-256とする。`artifact_size`と共に配送・cacheを検証する。
- Serverは検証済みfile集合から既存canonical pack規則でarchiveを生成し、そのbytesを一度保存する。取得時に再packしない。upload元ZIPのbytesと保存bytesが異なり得るため、両digestと保存sizeを応答する。
- planは両digest、package schema、exact Component world/API versionを固定する。Runtimeは受信bytesのhashに加え、package validatorで論理digestを検証する。

## 3. Workspace管理API

管理操作は既存のtyped `ServerApi`へ追加する。以下は採用するoperationの意味であり、別のPlugin専用HTTP frameworkは作らない。HTTP prefixは既存の`/api/w/{workspace}`とし、route/DTO/OpenAPI/clientは同じ宣言から生成する。

### 操作と競合

| 操作 | 契約 |
| --- | --- |
| `POST /plugin-uploads` | bounded binary `.yoi-plugin`を受け、caller/Workspaceに束縛した一時upload identityと検証結果を返す。install・enableはしない |
| `POST /plugins` | upload identity、Workspace内alias、operation keyでinstallationを作成。結果はinstallation/artifact identityと確定operationの識別子 |
| `GET /plugins` / `GET /plugins/{installation}` | Workspace-scoped bounded一覧・詳細。opaque cursor、page sizeの上限あり。guest codeは実行しない |
| `POST /plugins/{installation}/artifacts` | 新しい検証済みuploadからartifact追加。観測した最終catalog変更operationと新しいoperation keyが必要。既存plan/configは変更しない |
| `POST /plugins/{installation}/revoke` | 指定artifact digestの導入許可を失効。観測した最終catalog変更operation、理由、新しいoperation key。新規実行と既存実行の操作権限を撤回 |
| `POST /plugins/{installation}/uninstall` | 観測した最終catalog変更operationと新しいoperation keyで除去を要求。参照があれば拒否し、安全な阻害情報を返す |
| `GET /plugin-operations/{operation}` | 認可された同Workspace callerが保存結果を照合。結果不明のmutationを新しいkeyでやり直さない |

uploadはmanagement operationと分ける。巨大bytesをJSON/idempotency記録に埋めず、検証に失敗したuploadからcatalogが生じないようにするためである。upload identityは24時間で失効する一時資源で、同じcontentの再uploadは可能。失効後も既にcommitしたinstallationを消さない。usage/quotaは未確定uploadも含む。

catalog mutationのoperation keyはWorkspace・認証主体・操作kindに束縛し、canonical request fingerprintと保存結果をdomain更新と同じtransactionで確定する。fingerprintは対象、観測した最終catalog変更operation、内容digest、alias、理由など意味を変える入力を含み、生成された結果IDや秘密値を含まない。同じkey/入力には元の結果、異なる入力にはconflictを返す。replay時も現在の読取り権限を確認し、権限失効後に以前の結果を漏らさない。保存済みreplayは最新stateの観測ではない。

catalogの追加・失効・除去では、そのinstallationの最終変更を確定したoperation identityをtransaction内で比較してから更新する。時刻やstateだけでは、同じstateのままartifactが追加された後の古い削除判断を拒否できないためである。設定内容はconfig treeの実内容digestで比較し、別の汎用更新カウンタは導入しない。

configのenable/disable、artifact選択、接続binding、grant変更は既存Workspace config Preview/Commit/CASへ統合する。別のenable REST endpointを作って設定正本を二重化しない。Preview後のinstallation/schema変更はcommit時にも照合する。

### install・disable・revoke・uninstall

- installは導入可能なartifactを登録するだけで、Workerの開始やTool登録を行わない。
- updateとconfig disableは新規/replacementのplanに反映する。既存Workerのpinをhot reloadしない。即時停止が必要なら明示revokeを使う。
- revokeはinstallationとartifact digestの組に対して不可逆とする。同じ組を再許可して古い実行を復活させない。管理者が同じbytesを再利用する場合は新しいinstallationを明示作成し、新しいconfig/planを生成する。古いinstallationの失効tombstoneとpinは保持する。
- revokeは新規Host操作を拒否し、稼働中guestの取消・接続closeを要求する。既に外部へ送った効果の取り消しは保証しない。Remote Runtimeへ失効通知を送るだけでは認可が撤回された証拠にならない。保護操作のadmissionはownerでlive確認し、確認不能なら拒否する。stream接続はHostの失効監視で閉じ、監視を維持できない場合も継続しない。
- uninstallはconfig、active/pending/restorable Worker pinが参照する間は拒否する。revokeしてもpinを勝手に消さない。Worker retention解除・config変更・uninstallを別操作として扱う。
- installationの拡張データが残る場合もuninstallは拒否する。後述の明示purgeで消すか、データを保持したdisabled installationとして残す。
- artifact GCは全Workspaceの参照を確認する。DB参照より先に永続blobをpublishし、crash時の未参照blobは後で回収する。未publish blobをcatalogから参照させない。

参照検査・新規pin/config参照の作成・削除fenceは同じdomain transaction境界で競合を解決する。削除直前の単なるlist結果では判断しない。

HTTP errorは既存共通error envelopeへ合わせる。invalid=400、未認証=401、権限不足=403、不可視対象=404、最終catalog変更operation不一致/operation-key/参照競合=409、size=413、quota=429、owner unavailable=503を使う。他Workspace対象の存在をerror詳細から漏らさない。errorの`retryable`だけでmutationの新規再送を許可しない。

## 4. package宣言とABI version

新しい実行契約は`yoi:plugin@2.0.0`と`yoi:host@2.0.0`にする。JSON文字列だけのHost呼出し、数値handle、guest側WebSocket recvを変えるため、v1の意味を黙って差し替えない。

- 新しいmanifest schemaはv2とし、required worlds/interfaces、Tool schema、接続slot、拡張data schema、event subscriptionを静的に宣言する。schema contributionを得るためにguestを実行しない。
- Workspace設定はDCDLのclosed typed schemaで検証する。Toolのmodel-facing入力はJSON Schemaで定義する。JSON SchemaとDCDL schemaを同じものと仮定しない。
- Componentの実imports/exportsを静的に検査してmanifestと照合する。manifestの申告だけで未公開importをlinkしない。
- 初期実装はexact world/interface versionだけを受け入れる。semver rangeでHost APIを黙って置換しない。互換な追加は別versionとして明示サポートし、既存pinには元の契約を提供するかunsupportedで止める。
- 旧v1 packageはofflineで読めても新Runtimeでは実行対象にしない。v2向けrebuild/explicit installを要求し、汎用JSON adapterで旧権限経路を復活させない。
- PDK/template/validator/WITを一緒にv2へ更新する。今回の設計文書だけを根拠に既存v1 templateが新Runtimeで動くとは扱わない。

## 5. Component exports

Tool payloadだけはPluginごとにschemaが異なるため、schema検証済みUTF-8 JSONを運ぶ。それ以外の共通context、error、event envelope、Host request/resultはWIT record/variant/resourceで定義する。

以下は採用するinterfaceの形を説明する最小例で、現行WITへ追加済みという意味ではない。

```wit
package yoi:plugin@2.0.0;

interface tool {
  record invocation {
    invocation-id: string,
    tool-name: string,
    input-json: string,
    config-json: string,
    remaining-ms: u32,
  }
  record output {
    summary: string,
    content: option<string>,
    structured-json: option<string>,
  }
  record tool-error {
    code: string,
    message: string,
  }
  invoke: func(input: invocation) -> result<output, tool-error>;
}

world tool-plugin {
  export tool;
}
```

`invocation-id`はHost生成の追跡identityであり権限ではない。contextにServer token、host path、任意Workspace selectorを含めない。configはplanのnon-secret snapshotで、secret値は含めない。structured outputには宣言されたoutput schemaを要求し、Hostが検証する。

Tool worldは呼出しごとにfresh Store/instanceとし、compiled Componentは共有できる。Toolのglobal/static memoryを永続化や他呼出しとの通信に使えない。v2初期Toolは応答一回のbounded callとし、guestがmodelへ直接streamを注入するAPIは設けない。

Service/eventの機械的契約は[独立した`@2.2.0` world](wit-service/yoi-plugin-v2.2.wit)とする。既存`@2.0.0` HTTPS/loggingと`@2.1.0` Ticket/dataをexact importし、旧world/pinへの追加はしない。Service/event worldには`on-start(config) -> result`、`on-event(envelope) -> event-result`、`on-stop(reason) -> result`を設ける。いずれもfresh Storeで呼び、HostがWorker/installation/planに束縛したService resourceを保持する。ToolとServiceで共有する状態はHost経由の明示dataとし、guest memoryの残存を契約にしない。`status()`を呼んで診断のためにguestを実行せず、Hostが実行・接続・失敗の観測を返す。

`on-start`は設定検証とローカル初期化だけを行い、logging以外のHost作用を認めない。GETも副作用を持ち得るため、汎用HTTPや付随data書込みを初期化中に許可しない。外部event sourceの接続は全required Pluginの検証・導入成功後にHostが開始する。initialization失敗時に外部効果のrollbackを装わない。`on-stop`もloggingだけのbest effort通知で、resource解放の前提にしない。

## 6. Host importsとcapability lifetime

公開するinterfaceは機能ごとに分ける。汎用`invoke(method, json)`、汎用Server HTTP proxy、DB connection、host path、任意process実行APIは提供しない。

| interface | 操作の意味 |
| --- | --- |
| `connections` | 宣言slot名から、その呼出しに許可されたconnection resourceを取得 |
| `http` | connectionに束縛されたbounded request/response |
| `extension-data` | Plugin所有schemaのdataを対象object capability経由でread/CAS update |
| `logging` | bounded structured診断。model contextへの注入ではない |
| `workdir` | 後続で既存attachment-scoped operationを型付きにadapt。ambient filesystemは不可 |
| `websocket` | 後続のHost-owned driverへのsend/close。recvはevent配送で行う |

`connections.open(slot)`はWIT resourceを返す。slotはmanifestの宣言名で、Workspace設定が具体的な接続先とsecret参照にbindする。guestは任意credential IDや別Workspace IDを指定しない。resourceはStore/呼出し内だけで有効で、serialize・persist・別Workerへtransferできない。Serviceの長寿命接続自体はHost所有とし、各event処理時にfresh handleを発行する。

Hostは各importでcurrent invocation、installation、plan、付与範囲、対象資源、deadline/cancellationを照合する。lookup済みhandleを後続操作の無条件許可にしない。guestのComponent start sectionからの外部I/Oも禁止し、export呼出し用contextのないimportを拒否する。

planはconfig treeの内容digest、Profile projection digest、installation/artifact identities、接続binding identityとpolicyの内容digest、宣言schemaと実効grant、実行主体と委譲identityを固定する。管理操作を行った人の資格情報はguestの実行主体ではない。

| 変更・状態 | 既存plan/new export/restoreへの作用 |
| --- | --- |
| artifact追加、config enable/disable、Profileの選択・grant設定変更 | 新規/replacement planだけに適用。既存pinの集合や付与snapshotは変更しない |
| Workspace/対象資源のlive grant上限の縮小、明示的grant revoke、実行主体の委譲失効 | 既存snapshotとの積集合を直ちに縮小。次のadmission/Host操作を拒否し、進行中処理へ取消要求 |
| live grant拡大 | 古いsnapshotを広げない。追加利用には新規planが必要 |
| 接続binding失効、installation+artifactのrevoke | 新しいexport・接続・restoreを拒否。既存guest/driverを取消対象にする |
| 管理者の管理権限喪失 | 以後の管理操作を拒否。独立したWorker実行権限まで導入者の権限と同一視しない |
| ownerへ確認できない | fresh export、restore、保護操作・保存結果読取りを拒否。cached planを認可にしない |

各fresh exportの開始とrestoreにはServer ownerでinstallation/plan/実行主体のlive admissionを行う。pure Toolも例外にしない。既にadmit済みのローカル計算は取消・deadlineまで進み得るが、失効後の結果公開と追加Host作用は拒否する。初期Toolにoffline実行leaseは設けない。設定でのgrant変更とlive security grantの失効を混同しないよう、管理画面/APIの結果も適用対象を明示する。

WASIを一括linkしない。v2 worldに宣言したimportだけをallowlistし、ambient filesystem/network/environment/stdin/stdoutを与えない。clock/random等を追加する場合も個別のversioned interfaceとして明示する。PDKはこのimport集合だけでcompileできるようにする。

Wasmtimeのasync Host実装を使って待機中のexecutor threadを占有しない。ただしguestにexperimental async Component ABIを要求せず、初期WITは通常のcall/resultとする。Storeを並行利用せず、同一Service consumerへのevent処理を直列化する。

## 7. 外部HTTP接続

HTTPはURLを自由に受けるAPIではなく、connection resourceに対する相対pathのrequestとする。Server/Runtimeの資格情報を外部requestへ転用しない。

requestはmethod enum、absolute-path形式の相対path、query pair、許可されたheader pair、body bytes、要求timeoutを持つ。responseはstatus、許可されたheader、body bytesを持つ。4xx/5xxもHTTP応答として返し、transport errorと区別する。binaryはbyte listで運び、base64やJSON stringで二重に包まない。

初期Host interfaceの型は次の形にする。Tool worldにはこれらの必要なinterfaceを明示importし、未使用の機能を一括importするworldにはしない。

```wit
package yoi:host@2.0.0;

interface errors {
  enum code {
    invalid-input, denied, not-found, conflict, unsupported,
    quota-exceeded, deadline-exceeded, cancelled, unavailable,
    indeterminate, receipt-expired,
  }
  enum effect-observation { not-dispatched, response-observed, indeterminate }
  record host-error {
    code: code,
    message: string,
    diagnostic-id: string,
    effect: effect-observation,
  }
}

interface connections {
  use errors.{host-error};
  resource connection;
  open: func(slot: string) -> result<connection, host-error>;
}

interface http {
  use connections.{connection};
  use errors.{host-error};
  enum method { get, head, post, put, patch, delete, options }
  record pair { name: string, value: string }
  record request {
    method: method,
    path: string,
    query: list<pair>,
    headers: list<pair>,
    body: list<u8>,
    timeout-ms: u32,
    effect-key: string,
  }
  record response {
    status: u16,
    headers: list<pair>,
    body: list<u8>,
  }
  send: func(target: borrow<connection>, input: request) -> result<response, host-error>;
}
```

queryはHostがpercent-encodeして送る。path中への`?`埋込みを拒否し、query/headerの重複・順序は許可されたprotocol semanticsを保持する。guest指定の接続/転送制御headerは拒否し、Hostが必要なものを生成する。effect-keyは最大128 UTF-8 bytesの非秘密のopaque文字列とし、Hostのinvocationに束縛する。

- Workspace bindingはexact origin（scheme/host/port）、許可method、segment境界のpath範囲、secret参照、資源上限を持つ。packageはslotと要求範囲を宣言できるが、bindingを自己発行できない。
- 初期APIはHTTPSのみ。TLS検証を無効にできない。raw socket、任意proxy、Unix socket、CONNECT、upgrade、redirect自動追従は提供しない。
- pathはHostが一度canonicalizeし検査・送信に同じ値を使う。scheme/authority変更、userinfo、fragment、backslash、encoded separator/dot traversalを拒否する。prefixの文字列一致で`/api`に`/apix`を含めない。
- HostがDNS解決した全候補を検査し、許可したaddressへ接続を固定する。接続時の再解決で検査を迂回しない。TLSの名前検証とSNIは許可hostnameを使う。
- loopback、link-local、metadata endpoint、Server/Runtime管理endpointを拒否する。private LANはServer運用policyが許す範囲内でWorkspaceが明示bindingできる。Workspace管理権限だけで共有Hostの内部network全体を開かない。
- `Authorization`、Cookie、Host、proxy/hop-by-hop header等はguestが指定できない。接続policyで許可したnon-secret headerだけを受け付ける。credentialはHostが指定origin/pathへだけ注入し、responseのSet-Cookie等を返さない。
- credentialの値をguestへ返すAPIはない。ただし相手serverがsecretをechoする可能性まで完全に除去できるとは主張しない。credentialの配送先はWorkspaceが管理する信頼境界として扱う。
- planはsecret参照と接続policyの内容digestを固定し、secret値は保存しない。通常のsecret rotationは同じbindingのHost解決で反映できるが、宛先/権限変更は新規planにする。live失効・policy縮小は古いplanにも効く。
- request/responseはbuffered boundedとし、上限到達時は切断してlimit errorを返す。切断されたresponseを成功に見せない。圧縮展開後の上限も検査する。

### 外部副作用と再試行

HostはHTTP requestを自動再送しない。GETであってもprovider固有semanticsがあるため、transport層が勝手に再試行しない。redirectもguestにresponseとして返すだけで、別originへの追跡は新しい認可済みbindingが必要になる。

全HTTP requestに`(Workspace, installation, Worker, consumer-or-tool-call, logical-invocation, effect-key)`のscopeを使い、request fingerprint、送信admission、保存結果を記録する。consumerはServiceの場合の固定identity、Toolの場合は永続Tool-call identityであり、別consumerで同じevent-idを処理してもreceiptを共有しない。GETも例外にせず、methodだけから副作用なしを推定しない。同じkey/内容は保存結果を返し、異なる内容はconflict。guestがkeyを変えれば新しい作用なので、package内の再試行も自動的に安全になるわけではない。

fingerprintはinstallation/plan、接続binding identityとslotとpolicyの内容digest、method、正規化path、順序を保つquery/header、body hashを含む。secretの値、残りdeadline、attemptは含めない。saved responseはboundedでWorkspace保護データとして保持し、diagnostic logとは分離する。request body/secret headerの生値をreceiptへ保存しない。同一requestの保存結果を返す際にも現在のinstallation/接続/読取り権限を検証する。

invocation identityはHostの永続Tool-call/event記録に結び付け、同じTool-callの回復や同じconsumer/eventの再配送では固定する。guestがidentityを指定・変更できない。新しいユーザーの呼出しは新identityとする。返却済みTool結果は再実行せず既存結果を使い、未完了の回復を行う場合だけ同じeffect-key領域を使う。

送信前にdurable admissionを記録し、記録不能なら送らない。送信した可能性があるままtimeout/切断/crashした場合は`indeterminate`とし、同じkeyでも汎用Hostは再送しない。provider固有の照合・idempotency APIがある場合だけ、そのadapterの契約で回復できる。Tool結果が失敗でも外部効果が取り消されたとは表示しない。

保持期限を過ぎたeffect keyは未使用keyとして扱わず`receipt-expired`で拒否する。古いinvocation/eventの再開も同様で、再実行は明示的な新しい依頼にする。invocationの受理時にHostがreceipt照合期限を固定し、期限の過ぎたidentityには新しいeffect-keyを追加できない。期限情報/tombstoneは関連Worker/eventの保持期間中残し、bodyを削除しても未実行扱いに戻さない。receipt保持とevent再配送可能期間をHostで連動させ、exactly-onceを約束しない。

## 8. 共通error、出力、診断

Host errorは閉じたWIT variantで返し、少なくともinvalid-input、denied、not-found、conflict、unsupported、quota-exceeded、deadline-exceeded、cancelled、unavailable、indeterminate、receipt-expiredを区別する。guest business errorは宣言されたcodeとbounded messageで返す。未知guest codeはplugin-failedへ正規化し、Host errorの権限をguestが偽装できないようにする。

Host errorには安全なmessage、診断ID、外部効果の観測（not-dispatched / response-observed / indeterminate）を付ける。`response-observed`はbusiness成功や外部transaction commitの証明ではない。再試行の可否はerror文字列でなくoperation契約で判断する。trap、不正output、resource超過はHostが実行失敗として記録する。

Tool outputは通常のWorker Tool結果・履歴経路へ流す。Pluginがhidden prompt、System instruction、任意Session entryを直接差し込むAPIはない。structured JSONはschema検証し、不正・超過時に切り詰めたJSONを返さない。textの表示短縮はHost projectionで明示する。

loggingはlevel、event code、安全なmessageとscalar fieldsに限定する。raw request/response、secret、prompt、host pathを標準診断に保存しない。Pluginが受け取った外部データ自体はuntrusted contentであり、命令や権限として扱わない。

## 9. 付随データの契約

Ticket拡張の機械的契約は[追加world](wit-ticket/yoi-plugin-v2.1.wit)と[resource imports](wit-ticket/deps/yoi-host-v2.1/yoi-host-v2.1.wit)、`server-api::plugin_data`に置く。旧`@2.0.0` worldへのimport追加や保存pinへのgrant補完はしない。package保存formatのschema versionと、Component world/APIのexact versionは別の境界である。

Server拡張はdomain objectに関連付けるPlugin所有のtyped dataとし、既存tableへの任意column追加では実現しない。物理保存schemaはHostが管理する。`feature-storage`のnative migration/SQLite handleは信頼済みHost用でありguestへ公開しない。

addressはHost contextのWorkspace/installationと、認可済みobject resource、宣言collection、schema id/version、keyから決まる。schema id/versionも物理・論理namespaceに含め、新旧artifactが同じcellを別schemaで読み書きしない。guestにWorkspace selectorやraw table名を与えない。最初のobject kindはTicketとし、Hostが存在・Workspace・callerのread/write権限を検証したTicket resourceだけを使う。一般objectへの拡張はadapterと権限を追加して行う。

後続の`objects.resolve-ticket(ticket-ref, access)`はreadまたはextension-writeを要求し、現在のWorker主体・呼出元からの委譲とPlugin grantを検証して呼出し内だけ有効なWIT `ticket` resourceを返す。Tool入力のTicket refもevent envelopeのrefも単なる選択子であり、どちらも同じ解決を通す。ServiceにはService用grantを適用し、eventを受けたことだけでwriteを許可しない。削除済み・不可視・別Workspaceの対象はnot-foundとする。

`ticket-read.read(borrow<ticket>)`はread権限を再検査し、title、case state、content digest、bounded bodyとtruncated flagだけを返す専用read APIとする。thread、添付、認証・管理情報、将来増えるfieldを汎用serializeで公開しない。上限は64 KiB、完全本文が必要なら本APIではunsupportedとする。eventの削除通知自体は安全なmetadataとして配送できるが、削除済みTicket resourceをresolve/readできる意味ではない。

- packageはcollectionごとにschema id/versionとclosed typed schemaを宣言する。data valueのJSONはそのschemaで検証する。同installationの同collection/schema id/versionはrevoked artifactを含め同一schemaへ束縛し、artifact登録transactionで内容差を拒否する。exact artifactだけでは共有cellの意味を固定できないためである。value上限64 KiBはJSONのUTF-8表現（escapeを含む）のbyte数とし、listは最大100件、Remote envelopeの再escapeを含む有限response budgetを適用する。
- APIは`get`、`put(expected_last_write | never_written, operation_key, value)`、`delete(expected_last_write, operation_key)`、bounded `list(cursor, limit)`。任意query、SQL、cross-object transactionは公開しない。
- 同じcellへのput/deleteを確定した最終operation identityを、書込みtransaction内で観測値と比較する。CAS成功とoperation receiptを同じtransactionで保存し、自動読直しによるlast-write-winsへ変えない。同じ内容の再書込みや削除・再作成を時刻/内容hashだけで区別できないため、このoperationを比較する。
- deleteは最終delete operationのtombstoneを保持する。再作成はそのoperationを期待値にし、`never_written`を通さない。これによりABAを防ぐ。`get`は対象objectの認可成功後に限り、present(value, last-write)、never-written、deleted(last-delete)を区別する。不可視objectと不存在は同じnot-foundにする。
- data mutationのreceiptはHTTPと同じ実行scope（Workspace/installation/Worker/consumer-or-tool-call/logical-invocation）とguest operation-keyに束縛する。fingerprintに完全なcell address、put/delete種別、期待last-write、schemaとvalueのdigestを含める。異入力でkeyを再利用すればconflictとする。現在の対象認可確認後にreceipt照合をCAS検査より先に行い、成功済み同一要求を最新cellとの差で失敗にしない。照合期限・expired・tombstone保持はHTTPと同じinvocation規則を使う。
- `lookup-mutation(operation-key)`で同じinvocationの保存結果（committed: mutation identityと結果digest / not-recorded / expired）を現在の対象認可の下で照合できる。event再配送では先にreceiptを照合し、成功済みなら新しい値を読み直して同じkeyで別のputを作らない。ownerがnot-recordedを返した場合だけ現在のcellから要求を作れる。照合通信失敗はnot-recordedではない。外部HTTP・data・event ackを跨ぐ原子性は保証せず、handlerは段階ごとの保存結果で回復する。
- listはstable key順のbounded探索でありsnapshot全体の一貫性は保証しない。cursorはWorkspace/installation/object/queryに束縛し、不正・失効cursorを最初のpageに黙って戻さない。
- Ticketの通常fieldやworkflow判断をこのdata APIで変更できない。必要ならそのdomainの専用Host APIを別に設計する。
- 親object削除とPlugin data書込みはowner側の削除fenceで競合を解決する。削除後に新しい子dataを作らず、retention後にHostが回収する。
- schemaの破壊的変更は新しいschema version/namespaceとする。既存dataをguest起動時にmigrationしない。移行は新旧schemaを明示した認可付き操作として後続で設計し、初期APIでは自動変換しない。
- rollbackで旧artifactを使う場合も元schemaだけを読む。最新schemaへ読み替えない。
- revoke/disableではdataを消さない。Workspace管理者の明示purgeは先に全artifactをrevoke/fenceし、その後のtransactionで観測したinstallation最終catalog operationと、全cell（tombstone含む）のnamespace/key/最終write operation/deleted状態の順序付き集合digest・件数を期待値として比較する。新規writeだけでなく同一値の再書込みやdelete/recreateも区別し、汎用更新番号は導入しない。operation keyによるpurge receiptを残し、uninstallとの一操作への暗黙統合をしない。

## 10. commit済みeventとService

最初のhookはcommit済みeventの観察とする。PluginをServer transaction内で実行せず、元操作の成功をPluginの応答待ちにしない。domain ownerがtransactional outboxへeventを記録し、Hostが認可・投影して配送する。共有の無制限event busは提供しない。

最初のeventはTicket変更通知とし、envelopeはevent-id、event-type/version、occurred-at、対象resource reference、変更を確定したdomain eventのidentityと対象内容digest、correlation/causation identity、bounded typed payloadを持つ。全文や秘密fieldは載せず、必要な詳細は認可付きAPIで取得する。削除eventは削除済みreferenceと安全なmetadataのみとし、resource accessを復活させない。

subscriptionはmanifest宣言・Workspace grant・Profile選択から確定し、guestが任意event名を動的subscribeしない。consumerはWorker/installation/plan内のservice単位であり、別Workerに同じPluginを入れると別consumerになる。Workspace-wide singletonや分散consumer groupは暗黙に作らない。

- event-idは再配送でも固定、attemptはHost生成の観測値。consumer内はHost保存順に一件ずつ処理する。Workspace全体や別consumer間の全順序は約束しない。
- `event-result`はack、retry-after、rejectのvariant。ackはHostのcursor commitで確定し、guestのreturnだけでは確定しない。
- deliveryは保持期間内のat-least-once。crash/timeoutで再配送され得るため、外部作用はevent-id由来invocationとstable effect-keyを使う。payload/keyを変えた再送を同一作用として扱わない。
- retry-afterはeffectが結果不明でない場合のみ再配送を予約できる。indeterminateな外部効果を含むeventは要確認でconsumerを止め、汎用retryで再送しない。
- reject、繰返し失敗、queue上限、retention gapではconsumerをpausedとして可視化する。silent drop、無限retry、詰まったeventの無断skipはしない。復旧・明示skipはWorkspace管理操作として監査する。
- cursorが保持範囲から外れたらgapを返し、最新位置へ自動復旧しない。payload取得時にも対象への現在のread権限を再検査する。
- disableは新規planに作用し、既存consumerの即時停止はrevoke/Worker stopで行う。restoreは同じconsumer identity/pin/cursorを使い、replacementは新しいconsumerとして開始する。開始時点以前のeventを自動backfillしない。
- 同じconsumerの旧・新Runtimeが同時に処理しないようHost実行leaseでfenceする。receipt/cursor mutationもleaseを検証する。

WebSocketは別のHost-owned source driverがeventへ変換する。guestの`recv(timeout)` loopは復活させない。network frame自体にdurable delivery保証はないため、domain outbox eventと同じ保証を表示しない。必要なら外部protocolのsequence/resumeをbridge側で使う。

処理前の検証・拒否hookは初期APIに含めない。将来導入時は対象操作ごとに入力/返却値/timeout/failure policy/再入・副作用を契約化し、汎用interceptorとしてServer内部を公開しない。

## 11. 資源上限と終了

以下は新APIの初期defaultとして採用する。既存実装値の転載ではない。Hostは実効limitを起動前に固定して診断へ公開し、運用上限は縮小できる。guestから無制限や上限引上げを要求できない。

| 資源 | 初期default |
| --- | --- |
| Tool input / output、event payload | 各64 KiB（UTF-8/serialized bytes） |
| config、付随dataの1 cell | 各64 KiB |
| guest linear memory | 256 MiB / Store |
| Wasmtime fuel | 10,000,000 / export call |
| Tool/event export wall deadline | 30秒、上位caller deadlineが短ければそちら |
| start / stop hook | 5秒 / 2秒 |
| HTTP request body / response body | 各1 MiB |
| HTTP header総量 | 16 KiB、64 pairまで |
| HTTP request deadline | 15秒、残りcall deadline以下 |
| guest発行Host操作 | 同時4、累計32 / call |
| log総量 | 16 KiB / call、超過はdrop件数のみ記録 |
| Service待機queue | 256 event、満杯時は配送をpause |
| event再試行 | 最大5 attempt、待機上限5分、その後pause |
| event自動再配送・effect receipt照合 | 最低7日、期限後は明示expired/gap |
| 管理list | default 50、上限100件 |

fuelはCPU時間の精密尺度ではない。wall deadline、memory/table/instance/resource数のWasmtime limiterと併用する。HTTPとguest実行で別々に30秒ずつ延長せず、同じcall deadlineから残り時間を計算する。package最大size/file数は既存validator上限を再利用し、値の第二正本を作らない。Workspace全体の保存/queue/実行同時数はHostの有限quotaでadmissionし、個別上限だけで総量を無制限にしない。

終了順は新規admission停止、invocation取消、接続/driver停止、期限付きon-stop、登録lease解除、Store/一時resource解放とする。取消中に届いた結果で終了済みinvocationへ追加作用を発行しない。失効後のon-stopは外部write禁止。guestがstopに応答しなくてもHostはresourceを回収する。既に送信された外部効果はrollbackしない。

## 12. 初期範囲と契約検証

最初の実行APIはTool、connection-bound HTTPS、loggingに絞る。付随データとcommit済みeventは本書の設計を採用する後続interfaceであり、初期worldへ動かないstubを登録しない。WebSocket、Workdir、MCP transport、判断hookも未対応importとして明示拒否する。MCP専用のprocess/credential APIを先行して作らず、必要なHost capabilityを個別に追加する。

最初のvertical contract testは、Server operatorでないWorkspace管理者によるupload/install/config、plan生成、Remote/Embedded双方のWasmtime Tool実行、Host HTTPS、通常Tool履歴への結果、同一pin restoreを通す。mock guestだけでHost境界の証明を代替しない。

境界testでは少なくとも次を固定する。

- 同じblobでも別Workspaceのcatalog/plan/secret/dataを読めず、digestだけのfetchができない。
- v1/未対応world、manifestと実importの不一致、未宣言Tool、invalid input/outputを実行前または結果公開前に拒否する。
- path canonicalization、DNS rebinding、redirect、credential header、private/control-plane宛先を実際のHost adapterで検証する。
- upload/publish/DB commitの部分失敗、CAS競合、同一key再送、異入力key reuse、uninstallとpin作成の競合を検証する。
- effectの送信前失敗、送信後応答喪失、receipt期限、cancel/revokeを区別し、indeterminateから自動再送しない。
- fuel/memory/wall deadline、Host待機中の取消、trap、不応答on-stopでもresourceが回収される。
- 後続data/eventではCAS/ABA、親削除fence、schema rollback、再配送、retention gap、lease競合、外部効果不明時pauseを検証する。

採用する契約と実装手段を区別する。DBのtable名、内部trait名、物理blob配置、adapterのmodule分割は実装裁量だが、Workspace分離、pin、APIの型・失敗保証・再試行・拡張境界はこの契約を満たす必要がある。

# Web visual review

この文書は、Web UIを変更する実装者が完了前に行うvisual
reviewの必須手順を定義する。Visual reviewは任意のpolish工程ではなく、type
check、linter、testと並ぶvalidationである。

[`design-language.md`](design-language.md)はWorkspace
Webが満たす視覚とinteractionの共通規則を定義する。Product固有の構成は[`product-ux.md`](product-ux.md)を参照する。[`tools/web-ux/README.md`](../../../tools/web-ux/README.md)はcapture
toolの操作方法を定義する。この文書は、誰が、いつ、何を目視し、どの証拠を残さなければ実装完了とみなせないかを定義する。

## 実装者の責任

Visual
reviewは、後段のReviewerやユーザーへ最初の発見を委ねない。UIを変更した実装者自身が、同じ変更の中で次を行う。

1. 変更前をcaptureする。
2. 変更後を同じ条件でcaptureする。
3. screenshotとbrowser evidenceを自分で確認する。
4. 発見した問題を修正する。
5. 修正後を再captureして確認する。
6. 変更と無関係な既存問題は、根拠を付けて分離する。

Agentが実装者の場合、そのAgent自身が`ViewImage`または同等のVLM入力で実画面を確認する。captureを生成しただけ、別のAgentへ画像を渡しただけ、HTTP
200を確認しただけでは完了しない。

## 適用範囲

次の変更ではvisual reviewを必須とする。

- page、layout、navigation、Sidebar、Header、Console、dialog
- componentのmarkupまたはstyle
- color、typography、spacing、border、radius、shadow、theme token
- copy、label、status、empty/loading/error/permission表示
- responsive breakpoint、container、overflow、scroll
- 表示dataの追加、削除、並び替え、progressive disclosure
- icon、focus、keyboard interaction、motion
- UIへ影響するgenerated typeまたはAPI projection

型、Backend、CLI、文書だけの変更でrendered Web
UIが変わらない場合は省略できる。省略する場合は、UIへ影響しない理由をvalidation
evidenceへ一文で残す。

## 必須の確認条件

変更したsurfaceに関係する条件だけを選ぶが、都合のよい一画面だけに限定しない。

### Persona

権限やdataが表示を変える場合は、該当するpersonaを分ける。

- owner
- non-owner
- anonymous
- read-onlyまたは操作権限なし

owner表示だけでpermission-aware UIを承認しない。

### Viewport

最低限、次を確認する。

- desktop: `1440px`幅を基準
- narrow: Sidebarとmainが競合する幅。現在は`768px`を基準
- mobile: `390px`幅を基準

`320px`対応、table、long form、Console、Sidebar
foldなどが変更対象なら、その境界も追加する。media
queryのviewport幅だけでなく、Sidebarやpanelを差し引いた実際のcontent幅を確認する。

### Theme

color、border、background、focus、status、code、terminalを変更した場合はlightとdarkの両方を確認する。片方のthemeで読めることを、もう片方のcontrastの証拠にしない。

### Data state

該当するstateを明示的に用意する。

- representative data
- empty
- loading
- error
- permission deniedまたはread-only
- long title、long key、long error、複数行content
- collectionの最小件数と、実際に起こりうる多い件数

happy pathだけでcomponentを承認しない。

## 目視確認の観点

### 情報のまとまり

- 同じ判断に使う情報がproximity、alignment、typographyによって一つのgroupに見えるか。
- 別の意味を持つ情報が、十分な距離または一つの明確なboundaryで分離されているか。
- 親子、peer、metadata、actionの関係が、DOMを読まなくても判別できるか。
- card、background、border、shadow、余白を重ねて同じgroupingを重複表現していないか。
- page title、Header、breadcrumb、section
  heading、説明文が同じ意味を繰り返していないか。

### Separatorとborder

画面上のすべての線について、何と何を区切る線か説明できなければならない。

- Headerの終端、peer row間、section開始、table
  cellなど、線の役割が一つに定まっているか。
- 近接した二本の線が同じboundaryを重複表現していないか。
- 線の上下の余白から、その線が前後どちらのgroupに属するか分かるか。
- すべてのsectionへ機械的に同じ線を置き、hierarchyを平坦化していないか。
- borderがなくてもspacingだけで十分な箇所へ線を追加していないか。

### Spacingとdensity

- 余白がtoken scaleに沿い、近い関係ほど狭く、別groupほど広くなっているか。
- labelとvalue、headingとcontent、rowとrow、sectionとsectionの間に一貫した大小関係があるか。
- 線の前後へ同程度の大きな余白を置き、boundaryの所属を曖昧にしていないか。
- 初期viewportにprimary contentまたは主要操作が存在するか。
- 空きすぎた領域が情報不足やhierarchyの弱さを隠していないか。
- 密度を上げるためにfont、touch target、line heightを過度に縮めていないか。

### Wrapping、overflow、可変長content

- 実際に起こりうる最長のlabel、title、key、ref、error、翻訳で確認したか。
- 一行を前提とするcontrolが意図せず二行にならないか。
- button labelが折り返されたとき、row heightとalignmentが壊れないか。
- title、metadata、code、URLが隣接actionを押し出さないか。
- truncateする場合、完全な値へaccessできるか。
- horizontal
  scrollはtable、board、terminalなど、その意味を所有する領域だけに閉じているか。
- page全体へhorizontal overflowが発生していないか。
- `overflow-wrap`だけで問題を隠さず、可読性と比較可能性を維持しているか。

### Responsive layout

- DOM順、視覚順、keyboard順が一致しているか。
- Sidebar、Header、primary contentの順序がmobileでも目的に合うか。
- Sidebarだけで初期viewportを占有していないか。
- breakpoint直前と直後の両方で、content幅とcontrol配置が成立するか。
- viewport
  queryだけでなく、containerの実幅に応じて一列化またはscrollを選べているか。
- desktopの比較関係を、理由なくmobile cardへ変換していないか。

### Typographyとcopy

- route titleが画面上で一度だけ表示されているか。
- page purpose、componentの存在理由、設計意図を説明するmeta
  copyが残っていないか。
- proseが判断、warning、constraint、permission、recoveryのいずれかに実際に必要か。
- heading level、font size、weight、colorが同じhierarchyで揃っているか。
- metadataがbodyより強く見えていないか。
- uppercase、letter spacing、monospaceを装飾目的で多用していないか。

### State、permission、action

- loading、empty、error、permission denied、unavailableが見分けられるか。
- statusがcolorだけに依存していないか。
- 利用できないowner-only actionをdisabledで並べるのではなく、page
  compositionから除外できているか。
- filled primary actionが同じscopeに複数ないか。
- destructive actionが通常のprimary pathと誤認されないか。
- errorとrecovery actionが、失敗したsurfaceの近くにあるか。

### Interactionとaccessibility

- keyboard focusが全controlで見えるか。
- focus順が画面の作業順と一致するか。
- fold、disclosure、dialog、menuを操作した後にfocusが失われないか。
- hoverでしか得られない必須情報がないか。
- scroll region、icon-only control、status markerにaccessible
  nameまたはtextがあるか。
- reduced motionで意味が失われないか。

## 実行手順

### 1. Scenarioを決める

変更対象route、persona、viewport、theme、data state、capture
pointを列挙する。既存scenarioが目的を満たす場合は再利用し、満たさない場合は同じ変更内で追加または更新する。

### 2. 変更前をcaptureする

`tools/web-ux`を使い、変更前のsourceとscenarioを固定する。出力はrepositoryのproduction
sourceへ混ぜず、`target/web-ux`などの開発artifact領域へ置く。

### 3. 変更前を目視する

既存問題も含め、変更対象付近のgrouping、separator、spacing、wrapping、responsive、stateを確認する。変更前を見ずに、変更後の印象だけで改善を主張しない。

### 4. 実装する

発見した問題と、適用するUX規則を対応付けて修正する。screenshotだけを整えるfixture固有hackや、productionと異なるshell、CSS、data
authorityを作らない。

### 5. 同条件で再captureする

persona、route、viewport、theme、data
stateを揃える。変更後だけ別の好条件へ変えない。

### 6. Screenshotとreview contextを確認する

実装者自身が画像を開き、`review-context.json`も読む。次を確認する。

- document status
- visible UI error
- console error
- page error
- failed request
- accessibility snapshot
- screenshot hashとcapture point

HTTP 200でも画面に`401 Unauthorized`などが表示されていれば失敗とする。

### 7. Compareする

before/afterを比較し、差分が意図した領域に限定されているか確認する。pixel
differenceの大小だけで合否を決めない。意図しない移動、折返し、欠落、色変化を目視する。

### 8. Testとevidenceを揃える

source test、component test、E2E、type checkを実行し、visual
evidenceと一緒に記録する。test成功はvisual reviewの代替ではなく、visual
review成功もtestの代替ではない。

## 合格条件

次をすべて満たした場合だけvisual reviewを合格とする。

- 実装者自身がbefore/afterを目視した。
- 必要なpersona、viewport、theme、data stateを確認した。
- grouping、separator、spacing、wrapping、responsiveを確認した。
- visible、console、page、request errorが解決または明確にdispositionされた。
- 発見した新規問題を修正し、修正後を再captureした。
- 意図しないvisual regressionがない。
- artifact path、scenario、確認結果をhandoffへ記録した。

次は不合格である。

- capture commandが成功しただけ。
- screenshotを生成したが開いていない。
- desktopだけ、ownerだけ、happy pathだけを見た。
- 後段Reviewerまたはユーザーが目視する前提でhandoffした。
- 「好みの問題」として、説明できないseparator、spacing、overflowを未確認のまま残した。
- testまたはlinterが成功したため、visual reviewを省略した。

## Evidenceの記録

handoffには最低限、次を記録する。

```text
Visual review
- Scenario: <scenario path>
- Before: <run id or artifact path>
- After: <run id or artifact path>
- Personas: <list>
- Viewports: <list>
- Themes: <list>
- Data states: <list>
- Findings fixed: <bounded list>
- Known issues: <bounded list with disposition>
- Result: pass | fail
```

画像をTicketやGitへ大量にcommitしない。artifact pathとsource
revisionを結び付け、必要なreviewerが同じ条件を再現できるようにする。認証profile、cookie、token、secret、private
response bodyをartifactやhandoffへ含めない。

## Toolの位置付け

`tools/web-ux`はbrowser起動、scenario再現、capture、review
context、compareを提供する。Toolは視覚的な良否を自動決定しない。

Linterが構文規則を検査し、testが特定のbehaviorを検査するのと同様に、visual
reviewはrendered
resultの関係性を検査する。三者は補完関係であり、どれか一つで他を代替しない。

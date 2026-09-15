# Workspace Web UX・デザインルール

状態: レビュー用の正規ルール案

この文書は、`web/workspace`の情報設計、操作設計、視覚設計、responsive対応、accessibility、CSSの責務を定義する。本番画面はこの文書へ収束させる。現行実装は判断材料ではあるが、ルールそのものではない。この文書と矛盾する既存のマークアップやCSSは、別の有効なパターンではなく移行対象として扱う。

`docs/.local/workspace-web-pattern-candidate-v1.md`は比較検討に利用できるが、productionの設計authorityではない。

## 設計の立場

Workspace
Webは、Workspace内のリソースを起点に、権威ある状態を確認し、注意が必要な箇所を判断し、一度に一つの操作を行うための作業面である。宣伝用ダッシュボード、分析画面、Backend
DTOのraw viewerにはしない。

永続化されたBackend
recordを状態のauthorityとする。UIは入力途中のdraftを保持したり、操作準備を先行したりしてよいが、推測、cache、通信状態を現在のdomain
stateとして表示してはならない。

## ページモデル

各routeは、主要な目的を一つだけ持つ。その目的は「Ticketを選ぶ」「このWorkerを確認する」「Repository
accessを設定する」のように、動詞と対象を一つずつ使って表現できなければならない。一文で表現できない場合はrouteを分けるか、副次的な情報を段階的に開示する。

Workspaceのページは、次のいずれかを基本形とする。

1. **一覧またはboard** — リソースを探し、注意が必要な項目を見つける。
2. **詳細** — 一つのリソースを理解し、現在実行できる次の操作を行う。
3. **formまたはeditor** — 範囲の明確な設定変更を行う。
4. **Console** — 一つのWorkerを操作しながら、履歴と現在のcontrolを確認する。

同じ目的を支える場合に限り、一つのrouteで複数の基本形を組み合わせてよい。たとえばTicket詳細では履歴とworkflow
actionを併置できる。一方、Workspace landing
pageに無関係なRuntime、Repository、診断情報を並べてはならない。

## 画面枠とナビゲーション

- root layoutが唯一のapplication shellとsidebar containerを所有する。
- sidebar
  sectionは、Global、Workspace、Settings、route固有sectionの順に積み重ねる。
- 子routeは自分のscopeに属するlinkだけを追加する。親のnavigationを置き換えたり複製したりしない。
- Workspace scopeのlinkは、Workspace routeの中だけに表示する。
- navigation labelとURLにはcanonical resource
  keyを使う。内部UUIDをlabelやfallback linkとして露出しない。
- 存在しない、または閲覧できないWorkspaceを開いた場合は、その状態を明示する。別のWorkspaceへ暗黙に切り替えない。
- sidebarのfold状態はshellが所有する。navigation
  sectionが別のsidebar、独自の幅、fold controlを持ってはならない。

## 情報階層

### ページヘッダー

通常のページは、一つの簡潔なheader rowから始める。

- 左側にpage titleを置く。
- 右側のprimary actionは最大一つとする。
- route分類を繰り返すeyebrowは置かない。
- titleを言い換えただけの導入文は置かない。

subtitleを置けるのは、判断に影響する情報、分かりにくいscope、重要な制約を伝える場合だけとする。resource
keyは、detail
pageで識別に必要な場合にtitle付近へ置いてよいが、人が読むtitleより強く見せない。

### セクション

- sectionはDTOやdatabase fieldの順ではなく、ユーザーの作業順に並べる。
- primary actionと、その判断に必要な状態を初期viewportに置く。
- containerを追加する前に、spacing、alignment、typography、必要最小限のseparatorで階層を作る。
- scanningに役立つ場合だけsection headingを置く。page
  titleで意味が明らかな領域を`Overview`などで重ねて囲まない。
- list、board、table、form
  group、timelineは連続した作業面として扱う。項目ごとに独立したcardへ分解しない。
- 一つの境界は一度だけ表現する。nested
  border、背景差、大きなgap、shadowを重ねて同じ階層を表現しない。

### メタデータ

主要画面に表示するのは、リソースの識別、healthやattentionの判断、項目間の比較、次の操作の選択に必要な値だけとする。

次の値は通常、label付きのdetail、debug、copy affordanceの後ろへ置く。

- 内部ID
- schema、config、recordのrevision
- digestとfingerprint
- provider diagnostics
- authority source label
- raw requestまたはresponse payload

機械的な値にはmonospace familyを使う。labelや説明文にはsans
familyを使う。DTOに値が存在することだけを理由に画面へ表示してはならない。

## 段階的な開示

段階的な開示は判断負荷を減らすために使う。前提条件や現在の失敗を隠すために使ってはならない。

- primary viewにはidentity、current
  state、attention、次の有効なactionを表示する。
- 副次的な運用情報はplain section、side
  panel、またはlabel付き`<details>`へ置く。
- debug evidenceとraw metadataは、ユーザーが明示的に開いた場合だけ表示する。
- destructive controlは、ユーザーが該当flowへ入るまでprimary
  actionより弱く表示する。
- create formとedit
  formは、明示的なactionの後に表示する。継続編集が主目的でなければ、成功後はresource
  viewへ戻すかformを閉じる。
- disclosure
  labelは内容または操作対象を具体的に示す。名詞を伴わない`More`や`Advanced`を使わない。

blocking error、permission requirement、validation constraint、unsaved
changeを閉じたdisclosureへ隠してはならない。

## Authority、権限、操作

permissionはcontrolのenabled状態だけでなく、ページ構成そのものへ反映する。

- non-ownerにowner-only mutation controlを表示しない。
- readは可能だがmutationできない場合、有用なread viewだけを構成する。空のaction
  barや理由の分からないdisabled controlを置かない。
- permission
  boundaryの説明は現在の作業に影響する場所だけに置く。`owner only`をpage
  eyebrowとして使わない。
- endpointからdataを取得できたかどうかでpermissionを推測しない。権威あるpermission
  projectionを使う。
- domain stateとtransport
  stateを混同しない。たとえばWebSocketがopenであることは、Workerがidleまたはhealthyであることを意味しない。
- 現在の権威あるstateで有効なactionだけを利用可能として提示する。
- destructive
  actionには対象リソースと結果を明記する。即時に復元できない場合は意図的なconfirmationを要求し、通常のprimary
  pathと視覚的に区別する。

page headerまたはform action group内のfilled primary
actionは一つだけとする。secondary actionはbordered actionまたはtext
actionとする。destructive actionは、最後の破壊確認段階を除きfilledにしない。

## 読み込み中、空、エラー、利用不能

dataを持つsurfaceは、意味のある状態を個別に扱う。

### 読み込み中

- data取得中もpage frameとsection位置を維持する。
- scopeが明らかでない場合は、何を読み込んでいるか示す。
- page全体を中央spinnerへ置き換えてlayout shiftを起こさない。
- spinnerだけに意味を持たせず、accessibility上のlabelも与える。

### 空

- absenceは`No tickets`のように直接表現する。装飾的なillustrationや長い説明文を置かない。
- permissionがあり、実際に利用できる場合だけ、次のactionを一つ提示する。
- recordが存在しない状態と、recordを読み込めなかった状態を区別する。

### エラー

- field validationは対象fieldの近くに置く。
- operation errorは失敗したactionの近くに置く。
- section load errorは対象section内に置き、利用できる他のsectionは残す。
- routeの主要目的を果たせない場合だけroute-level errorにする。
- diagnosticsはboundedかつactionableにする。payload、secret、内部traceをdumpしない。
- 新しく発生し、即時の注意が必要なerrorには`role="alert"`を使う。常設の説明文をalertにしない。

### 利用不能と権限不足

resource unavailable、capability missing、permission
deniedは異なる状態として扱い、copyも分ける。利用できない具体的なcapabilityまたは必要なpermissionと、安全な次のactionがあればそれを示す。`403`をempty
listとして表示したり、別のWorkspaceへredirectしたりしない。

## 視覚設計

### 基本姿勢

Workspace Webは、分離したwidgetの集合ではなく、一つのcontrol
surfaceとして見えるようにする。情報のgroupingには、まずspacing、typography、text
contrastを使う。border、rounded rectangle、shadow、filled
panelは、spacingだけでは伝わらない意味のある境界に限定する。

### カラーパレット

colorは`app.css`のCSS custom propertyをauthorityとし、OKLCHで定義する。light
modeとdark modeは`prefers-color-scheme`を通じて同じsemantic tokenを使う。

ルール:

- page backgroundとlayout surfaceはchroma zeroの`oklch(... 0 0)`を使う。
- primary textとcode textは、ほぼneutralなwarm colorとする。CSS
  OKLCHはsaturationではなくchromaを直接扱うため、「約5%
  saturation」の意図は`C = 0.01`から`0.012`程度の小さなwarm chromaで表現する。
- muted textは新しいhueを増やす前にlightnessとchromaを下げる。
- accentとstatus colorはsemantic
  exceptionとする。state、focus、navigationを示すために使い、container装飾には使わない。
- Workspace Web componentへraw hexまたはrgb
  colorを追加しない。必要な場合は`app.css`へsemantic tokenを追加する。

基本token:

```css
--bg
--bg-raised
--bg-subtle
--line
--line-strong
--text
--text-strong
--text-muted
--text-faint
--code
--accent
--success
--warning
--danger
```

### 表示面と操作部品

- structural surfaceにshadowや装飾目的のradiusを使わない。
- bordered parentの内側にbordered childを置かない。
- rowは一つのseparatorと、必要に応じたhoverまたはselected backgroundで表現する。
- button、input、select、textareaなどのinteractive
  controlだけに`var(--radius-soft)`を使う。
- resource key、link、status、table cell、Ticket、sectionをpillにしない。
- statusは小さなmarkerとtextで表現する。markerは`aria-hidden`とし、textに意味を持たせる。
- laneやsectionがすでにstateを示している場合、row内で同じstateを繰り返さない。
- colorとspacingにはsemantic tokenだけを使う。literal colorや局所的なspacing
  scaleを追加しない。

## タイポグラフィと文言

通常のWorkspace pageでは、三段階のtype hierarchyを使う。

| 用途                                   | size / line height | 使用箇所             |
| -------------------------------------- | ------------------ | -------------------- |
| Page title                             | `24px / 32px`      | routeごとに一つ      |
| Body、control、row、section title      | `14px / 20px`      | contentとinteraction |
| Metadata、table heading、machine value | `12px / 16px`      | 副次情報と機械的な値 |

別のfont sizeを増やす前に、配置、spacing、weight、muted
colorでhierarchyを作る。Consoleとterminal outputは、密度が必要な場合にmetadata
sizeのmonospace
styleを使ってよい。より多くのdataを表示するためにtextを`12px`未満へ縮小しない。

copyのルール:

- 具体的な名詞と動詞を使う。
- Workspace、Ticket、Objective、Repository、Runtime、Workdir、Worker、Merge
  Requestなど、ユーザーに見えるdomain nameを使う。
- identifier、API名、type名は、正確性が必要な場合に原文を維持する。
- page title、navigation label、明らかなcontrol behaviorを説明文で繰り返さない。
- proseを追加するのは、判断を変える情報、結果へのwarning、permissionまたはvalidation
  constraint、error recoveryを示す場合だけとする。
- empty copyとstatus copyは短く、事実を直接書く。
- buttonは`Add repository`、`Save settings`、`Stop Worker`のように操作を表す。`Submit`、`OK`、装飾的なcategory
  labelを避ける。
- statusとattentionをcolorだけで表現しない。
- uppercase labelは小さなmetadata labelに限る。大きなall-caps blockを置かない。

## リソースごとのパターン

### Tickets

- boardを一つの連続した横方向の作業面として扱う。
- laneは控えめなheadingと、隣接laneとの一つのseparatorで区切る。
- Ticketはlane内のflat rowとし、独立したcardにしない。
- Ticket key、title、優先判断を変えるmetadataだけを表示する。
- blockerとattentionはtextでも示す。
- narrow viewportでは、Ticketをmobile cardへ変換せず、label付きhorizontal scroll
  regionでlane比較を維持する。

### リソース一覧

- 同じfieldをリソース間で比較する場合、flat listまたはtableを使う。
- 人が読むnameとcanonical keyを最初のcolumnに置く。
- columnは判断に必要な値だけに絞る。provider detailとrevisionはresource
  detailへ移す。
- narrow viewportでcolumnを消すと比較できなくなる場合、label付きhorizontal
  scroll regionを使う。それ以外は一つのrowを明確なlabel付きでstackする。

### 詳細ページ

- identity、current state、attentionを最初に置く。
- narrativeまたはhistoryをprimary
  columnに置く。actionを同時に確認する必要がある場合だけ、bounded secondary
  regionを併置する。
- relation、target、review、workflowが別のauthorityを持つ場合、それぞれを独立したsectionにする。
- raw metadataは閉じたdetailまたはdebug disclosureへ置く。

### Settingsとフォーム

- 一つのSettings routeは一つの設定関心事を所有する。
- edit actionの前または同時に、現在有効な値を示す。
- labelはcontrolの直上に置く。
- help textはfield nameの説明ではなくconstraintを示す。
- 一つの判断に必要なfieldをgroup化する。fieldやgroupごとにcardで囲まない。
- narrow screenでは一列にする。複数columnは、同時に比較または決定する値に限る。
- form内でfilled actionにするのはsave actionだけとする。

### Worker Console

- transcriptをprimary surfaceとする。
- direct SubWorkerを表示している場合も、Composerとrun
  controlは親Workerへbindしたままにする。
- OverviewとNormalはpresentationだけを変え、authorityを変えない。
- command outputはterminalに近いcompactな表示とし、prose messageから分離する。
- live stateは権威あるWorker snapshotから導出する。connection stateやtransient
  eventで置き換えない。
- hidden reasoningとraw system promptはdebug modeでも表示しない。

## レスポンシブ対応

レスポンシブ設計では、すべてを同じstackへ変換するのではなく、作業上の関係性を維持する。

- desktopのpage paddingは`32px`、`760px`以下では`16px`とする。
- page全体にhorizontal scrollを発生させない。board、table、code
  view、terminalは、自分の範囲内でbounded horizontal scrollを所有してよい。
- side-by-side表示が同じ判断を支えなくなった場合、detail
  gridまたはformを一列にする。
- stack化によってworkflowやcolumn
  relationshipが失われる場合、比較面を横方向に維持する。
- primary actionを初期viewportから追い出さない。
- touch targetの操作性を維持し、overflow解消のためにtextを縮小しない。
- 長いkey、ref、diagnosticはwrapまたはtruncateし、完全な値へaccessできる手段を用意する。

## アクセシビリティ

- `main`、`nav`、`section`、`table`、`form`、`fieldset`、`label`、`button`などのsemantic
  elementを使う。
- すべてのcontrolに明示的なaccessible
  nameを与える。placeholderをlabelとして使わない。
- keyboard focusをsemantic accent outlineで常に見えるようにする。
- tab orderを画面上の順序と作業順に合わせる。
- icon-only controlには、対象リソースを含む具体的な`aria-label`を付ける。
- 重要なloadingとoperation statusにはboundedな`aria-live`
  regionを使う。streaming noiseを繰り返しannounceしない。
- error、warning、success、selected、runningは、color以外のtextまたはsemantic
  distinctionを持つ。
- 必要なhorizontal scroll regionはfocus可能にし、accessible nameを付ける。
- light schemeとdark schemeで同じsemantic tokenを使い、component-local color
  overrideなしで可読性を保つ。
- motionは任意かつ短くし、意味の必須条件にしない。繰り返しまたは連続animationではreduced-motion
  preferenceを尊重する。

## CSSの責務

`app.css`はglobal-onlyとし、次だけを所有する。

- font import
- cascade layer order
- semantic design token
- resetとbase element style
- application-level layout helper
- 無関係なfeature間で共有する小さなprimitive

featureとpageのstyleは、`workspace-pages.css`、`tickets.css`、`workers.css`、`settings.css`、`sidebar.css`など、その領域を所有するstylesheetへ置く。共有するvisual
ruleはliteral valueのcopyではなくglobal tokenで表現する。

Svelte-local
styleは、behaviorとstyleを分離できず、再利用もしない場合に限る。広範なpage
styleを`app.css`へ戻したり、component-localなcolor、spacing scale、font、z-index
systemを作ったりしない。

新しいUIを追加するときは、次の順に判断する。

1. semantic HTMLと、この文書のpage modelから始める。
2. global tokenと既存のfeature-owned patternを再利用する。
3. spacingとtext contrastでhierarchyを作る。
4. boundaryが意味を持つ場合だけborderを使う。
5. background fillはpage-level surface、selection、attention、read-only
   codeまたはrecord bodyに限定する。
6. 新しいcolorが必要ならOKLCH tokenとして追加し、既存のsemantic
   tokenで表現できない理由を残す。

## 避けるパターン

次のパターンを設計慣習として追加または維持しない。

- 無関係なaction cardで構成されたdashboard風landing page
- card-within-cardまたはpanel-within-panel
- hierarchyを作るためだけのshadowや大きなradius
- `Delivery`、`Workspace resources`、`owner only`などのeyebrow
- page titleで示したリソースを管理する画面だと繰り返すlede
- statusまたはresource keyのpill
- laneですでに示したstateをrow内で繰り返す表示
- DTO順に全fieldを露出するpage
- primary viewに置かれた内部ID、revision、digest、fingerprint、authority label
- non-ownerに表示されたowner-only disabled control
- loading、empty、error、unavailable、permissionを一つのgeneric
  messageへまとめる表示
- page全体をoverflowさせる固定desktop grid
- 比較関係を壊す任意のmobile card化
- color-only state、placeholder-only label、見えないkeyboard focus、accessible
  nameのないicon-only control
- 二つ目のsidebar、route-local application
  shell、親navigationを置き換えるsection
- component-localなliteral color、spacing scale、font、z-index system

## レビューと変更手順

Workspace Webの意味のある変更では、`tools/web-ux`をrepeatable browser
captureの手段として使う。

1. representative personaとviewportで現在のrouteをcaptureする。
2. screenshotと`review-context.json`を確認する。visible
   UI、console、page、HTTP、scenario failureを含めて判定する。
3. UX上の具体的な問題と、違反しているルールを記録する。
4. 最小で一貫したproduction変更を行う。
5. 同じpersona、route、viewportで再captureする。
6. before/after evidenceを比較し、関連するsource test、UI test、E2E
   testを実行する。

HTTP document
statusが200でも、画面に`401 Unauthorized`などのerrorが表示されていれば成功とはみなさない。authentication
profileはorigin-boundであり、別のhostまたはport用に取得したprofileを暗黙に再利用しない。

Design reviewでは、最初にユーザーのtask、次に情報階層とstate
handling、最後にvisual polishを評価する。test通過やscreenshot生成だけではUX
acceptanceを満たさない。

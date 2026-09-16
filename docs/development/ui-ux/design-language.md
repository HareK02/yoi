# Workspace Web Design Language

この文書は、Workspace Webをどう知覚させ、情報、状態、操作をどの視覚文法で表現するかを定義する。Product固有のresource構成は[`product-ux.md`](product-ux.md)、application shellとSidebar slotの実装構造は[`application-architecture.md`](application-architecture.md)、CSSとsource配置は[`../../../web/workspace/README.md`](../../../web/workspace/README.md)をauthorityとする。

一つの規則は、以下の各章のうち最も具体的な一箇所だけをauthorityとする。別の章で同じ禁止や手段を再掲せず、必要な場合はauthorityとなる章を参照する。

この文書はrendered resultが満たす設計規則を定義する。実装者自身がbefore/afterをcaptureして完了判定する手順は、[`visual-review.md`](visual-review.md)をauthorityとする。

## 1. Principles

この章は具体的な見た目や配置を定めない。複数の規則や要求が競合したときに、どちらを優先するか判断するための原理だけを定める。

### Task before explanation

現在のtaskと判断を、網羅的な説明、metadataの露出、装飾より優先する。UIはユーザーが次の有効な操作へ到達するまでの距離を短くする。

### Meaning before decoration

視覚上の差は、情報の重要度、関係、操作可能性、または状態の違いを伝えるためにだけ使う。意味を説明できない視覚表現は追加しない。

### Authority before convenience

UIの都合でdata、state、permission、利用可能なactionを推測しない。表示とaffordanceは、現在のauthorityが確定している内容に一致させる。

### Complexity follows need

複雑さを消すのではなく、判断に必要になる位置と時点で開示する。初期表示では、現在のtaskと競合する詳細を前面へ出さない。

## 2. Semantics / Grammar

この章は、Workspace Webで情報、操作、状態を何として扱い、どう表現するかを定める。

### Information structure

#### Priority and focus

- 情報はprimary、secondary、technicalの順に読む構造にする。
- 一つのscopeでは、primary resource、current attention、next actionのいずれか一つを最初のfocusにする。
- 重要度は最初に配置順、次にsize、weight、contrastで示す。colorや装飾だけで重要度を作らない。
- 同じscopeに最も強いheading、filled action、attention表現を複数置かない。
- metadataと補助操作は、主要なidentityやcurrent stateより強く見せない。
- accent、filled action、強いcontrast、motionを複数箇所で競合させない。
- normal stateは静かに保ち、attention表現は現在判断が必要な例外へ限定する。
- focusを作るために周囲の情報を過度に薄くしたり、小さくしたりしない。

#### Page and section structure

route titleはshell headerとmain contentを通じて一度だけ表示する。

- 標準headerまたはbreadcrumbが現在routeを十分に識別している場合、main contentは最初の操作対象またはsectionから始める。
- shell headerがscopeだけを示し、main contentの対象を識別できない場合に限り、main側へ一つのtitleを置く。
- primary actionは、そのactionがpage全体の主要操作である場合だけtitleと同じrowへ置く。
- route分類、breadcrumb、header titleをeyebrow、heading、keyで繰り返さない。
- subtitleとledeは原則として置かない。

一つのsectionは、一つの目的、ownership、または読み順を持つ。目的が異なる内容を同じsectionへ入れず、同じ目的の内容を見た目だけで複数sectionへ分けない。Section間とsection内の関係は次のGrouping and separationに従い、具体的なcueは`Primitives / Tokens`だけから選ぶ。

#### Grouping and separation

- 同じ判断や操作に使う情報は、一つのまとまりとして知覚できるようにする。
- groupの単位は、共通のownership、behavior、またはユーザーが行う一つの判断から決める。data sourceやDTOが同じという理由だけでgroup化しない。
- label、value、help、error、actionの対応関係を、別の説明を読まなくても追えるようにする。
- 異なるgroupの境界を知覚でき、その強さが意味上の距離と一致するようにする。
- 境界は何と何を分け、どのgroupの開始または終端を示すか説明できるようにする。
- 一つのgroupまたは境界へ複数の視覚cueを重ねず、異なるhierarchyの境界をすべて同じ強さで表現しない。

#### Metadata

主要表示に置くのは、identity、healthまたはattention、比較、次の操作に必要な値だけとする。

次の値は通常、label付きのdetail、debug、copy affordanceの後ろへ置く。

- 内部ID
- schema、config、recordのrevision
- digestとfingerprint
- provider diagnostics
- authority source label
- raw requestまたはresponse payload

機械的な値にはmonospace familyを使う。DTOに値が存在することだけを理由に表示しない。

#### 段階的な開示

- primary viewにはidentity、current state、attention、次の有効なactionを表示する。
- 副次的な運用情報はplain section、side panel、またはlabel付き`details`へ置く。
- debug evidenceとraw metadataは、ユーザーが明示的に開いた場合だけ表示する。
- destructive controlは、ユーザーが該当flowへ入るまでprimary actionより弱く表示する。
- create formとedit formは明示的なactionの後に表示する。
- blocking error、permission requirement、validation constraint、unsaved changeを閉じたdisclosureへ隠さない。

### Interaction and state

#### Affordance

- actionにはbutton、navigationにはlink、選択にはcontrolを使い、役割を見た目とsemantic elementで一致させる。
- interactive elementは、通常、hover、focus、active、disabledを区別できるようにする。
- staticなstatus、key、metadataをbuttonやpillの形にしない。
- clickable rowは、click可能な範囲と遷移先を一つにする。
- permissionがない操作をdisabled controlとして見せることでaffordanceを偽らない。

#### State representation

- 権威を持つdata sourceのstateをそのまま表示し、UIの都合でunknown、stale、unavailableを推測で埋めない。
- stateは対象resourceまたはoperationの近くに置き、何のstateかを位置関係で判別できるようにする。
- stateはmarkerとtextで示し、colorを補助に使う。
- loading、empty、error、permission denied、unavailable、staleを別の状態として扱う。
- 非同期operationでは、開始、処理中、完了、失敗を見通せるようにする。

#### Authority、権限、操作

- permissionはcontrolのenabled状態だけでなく、表示構成そのものへ反映する。
- readは可能だがmutationできない場合、有用なread viewだけを構成する。
- endpointからdataを取得できたかどうかでpermissionを推測しない。
- domain stateとtransport stateを混同しない。
- current stateで有効なactionだけを利用可能として提示する。
- destructive actionには対象と結果を明記し、復元できない場合は意図的なconfirmationを要求する。
- 同じscopeのfilled primary actionは一つだけとする。

loading、empty、error、unavailableは同じfallbackへまとめず、それぞれ次の表現を使う。

#### Loading

- page frameとsection位置を維持する。
- scopeが明らかでない場合は、何を読み込んでいるか示す。
- page全体を中央spinnerへ置き換えない。
- spinnerだけに意味を持たせない。

#### Empty

- absenceを直接表現する。
- permissionがあり、actionが実際に利用できる場合だけ、次のactionを一つ提示する。
- recordが存在しない状態と、読み込めなかった状態を区別する。

#### Error

- field validationは対象fieldの近くに置く。
- operation errorは失敗したactionの近くに置く。
- section load errorは対象section内に置き、利用できるsibling sectionは残す。
- routeの主要目的を果たせない場合だけroute-level errorにする。
- diagnosticsはboundedかつactionableにする。

#### Unavailableとpermission

resource unavailable、capability missing、permission deniedを別の状態として扱う。`403`をempty listとして表示したり、別scopeへredirectしたりしない。

## 3. Language / Copy

UI copyの語彙、長さ、説明責任を定める。

- 具体的な名詞と動詞を使う。
- identifier、API名、type名は正確性が必要な場合に原文を維持する。
- title、navigation label、明らかなcontrol behavior、componentの存在理由を説明文で繰り返さない。
- proseを追加するのは、判断を変える具体的な情報、warning、permission、validation constraint、error recoveryに必要な場合だけとする。
- empty copyとstatus copyは短く、事実を直接書く。
- buttonは操作を表す。`Submit`、`OK`、装飾的なcategory labelを避ける。

## 4. Cross-cutting requirements

viewport、input modality、支援技術にかかわらず、すべての表現とcomponentが満たす要件を定める。

### Responsive

- page全体にhorizontal scrollを発生させない。
- board、table、code view、terminalは、自分の範囲内でbounded horizontal scrollを所有してよい。
- side-by-side表示が同じ判断を支えなくなった場合、一列にする。
- stack化によって比較関係が失われる場合、比較面を横方向に維持する。
- primary actionを初期viewportから追い出さない。
- touch targetの操作性を維持し、overflow解消のためにtextを縮小しない。
- 長いkey、ref、diagnosticはwrapまたはtruncateし、完全な値へaccessできる手段を用意する。

### Accessibility

- semantic elementを使う。
- すべてのcontrolに明示的なaccessible nameを与える。
- 視覚的なfocusとkeyboard focusを一致させ、常に見えるようにする。
- tab orderを画面上の順序と作業順に合わせる。
- icon-only controlには具体的な`aria-label`を付ける。
- stateはcolor以外のtextまたはsemantic distinctionを持つ。
- 必要なhorizontal scroll regionはfocus可能にし、accessible nameを付ける。
- lightとdarkで同じsemantic tokenを使う。

## 5. Primitives / Tokens

色、文字、間隔、形、motionの最小単位を定義する。

### Color / Theme

colorはCSS custom propertyをauthorityとし、通常のWorkspace colorはOKLCHで定義する。lightとdarkは同じsemantic tokenを使う。

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
--accent-muted
--success
--warning
--danger
--interactive-hover
--interactive-selected
--shadow-overlay
```

- backgroundとlayout surfaceはneutralにする。
- muted textは新しいhueを増やす前にlightnessとchromaを下げる。
- accentとstatus colorはstate、focus、navigationへ限定する。
- componentへraw colorを追加せず、semantic tokenを使う。
- 意味が重なるtoken aliasを作らない。

ConsoleとterminalのANSI paletteは専用tokenを使い、通常のstatusやformへ流用しない。

### Typography

| 用途                                   | size / line height | 使用箇所                      |
| -------------------------------------- | ------------------ | ----------------------------- |
| Route title                            | `24px / 32px`      | shellまたはmainのどちらか一方 |
| Body、control、row、section title      | `14px / 20px`      | contentとinteraction          |
| Metadata、table heading、machine value | `12px / 16px`      | 副次情報と機械的な値          |

```css
--font-sans
--font-mono
```

別のfont sizeを増やす前に、配置、spacing、weight、muted colorでhierarchyを作る。textを`12px`未満へ縮小しない。

### Spacing / Shape

```css
--space-1 /* 4px */
--space-2 /* 8px */
--space-3 /* 12px */
--space-4 /* 16px */
--space-5 /* 24px */
--space-6 /* 32px */
--radius-soft
```

- groupingはspacingとalignmentを最初のcueとする。
- separationにはspacing、line、surface backgroundのいずれか一つをprimary cueとして選ぶ。
- 汎用的な`Card` primitiveは設けない。
- nested surfaceはparentと同じ境界表現を繰り返さない。
- shadowは通常flowから浮くmenu、popover、Tooltipなどの一時的overlayだけが使用できる。
- radiusはinteractive controlだけが使用できる。

### Motion

- motionは任意かつ短くし、意味の必須条件にしない。
- 繰り返しまたは連続animationではreduced-motion preferenceを尊重する。

## 6. Components / Patterns

実際のUIを、上のprinciple、grammar、communication、cross-cutting requirement、tokenから組み立てる。完成pageを固定templateにしない。

### Bevel / Structural edge

- raised edgeは、周囲より前にあるcontrolまたはsurfaceを表す場合に使う。
- inset edgeは、入力を受け取る領域または周囲より奥にあるsurfaceを表す場合に使う。
- ridgeとgrooveは構造上の境界を表すために使い、status、attention、単なる装飾には使わない。
- spacingやalignmentだけで関係を十分に示せる場合、Bevelを追加しない。
- Bevel自体へbackgroundやsurface toneを与えず、必要なbackgroundは内側のsemantic childが持つtext/content areaへ適用する。
- 隣接するsurfaceでは、実際に境界となる辺だけを見せ、存在しない境界やcornerを装飾目的で追加しない。
- standaloneな閉じた領域は、領域自体をBevelで囲う。ページ全体を分割するHeaderとSidebarはclosed boxとして隣接させず、DesktopではHeaderのbottom edgeとSidebarのright edgeだけを有効にして内部境界を示す。viewport外周に重なるtop／left edgeは描画しない。
- `BevelLine`は開いた領域内の構造separatorだけに使う。同じlayerではheading、本文、Lineの端を揃え、Lineだけに端方向のpaddingやmarginを追加しない。
- 内側のlayerを分離するときは、親layoutがそのlayer全体へinline方向の余白を与え、content、row、Lineを一緒に内側へ移す。Lineだけを短くして階層を表現しない。
- 外周border、focus ring、status marker、tableの意味的なgridはBevelや`BevelLine`へ置き換えない。

### Sidebar

- Sidebarは現在位置と利用可能なscopeを一つのnavigation hierarchyとして示す。
- desktop幅は`clamp(220px, 20vw, 280px)`を基本とする。
- navigation contentだけをscrollさせる。Desktopのfold controlはSidebar下部、Mobileのfold controlはHeaderに置く。
- MobileでSidebarを表示するときはHeader下の残りviewport全体をSidebarに使い、main contentと同時表示しない。
- navigationとscope controlだけを置く。
- navigation linkはsidebar幅全体を使うflat rowとする。
- active stateは一つのcueと`aria-current="page"`で示す。
- iconだけでlabelを置き換えない。
- fold controlは一つだけ置き、現在stateに対応するaccessible nameを使う。
- narrow viewportでも同じSidebarの意味とnavigation hierarchyを維持する。

### Tooltip / Contextual help

- operationやtable全体を説明する常設proseを置く前に、button labelとcolumn headingを明確にする。
- labelだけでは表現しきれない補助説明は、対象へbindしたTooltipで表示する。
- pointer hoverとkeyboard focusの両方で開く。
- triggerとTooltipは`aria-describedby`で関連付ける。
- hoverは短いdelayを持たせ、focusでは直ちに表示する。
- `Escape`で閉じ、triggerからfocusを奪わない。
- 複数paragraph、form、link、buttonを入れない。
- error、permission、validation constraint、不可逆操作の結果、current stateをTooltipだけへ隠さない。
- touch環境でも同じ説明へ到達できるようにする。

### 汎用component pattern

- **Action**: `primary`、`secondary`、`text`、`destructive`、`disabled`を同じ高さ、padding、focus contractで表現する。
- **Form field**: label、control、constraintまたはhelp、field errorの順に配置する。
- **Status**: markerと短いtextを一組にする。
- **Feedback**: loading、empty、operation error、permission boundaryを別の状態として表現する。
- **Resource row**: keyまたはname、判断に必要なmetadata、attentionまたはstate、一つのdestinationを置く。
- **Comparable data**: 同じfieldを比較する場合だけtableを使う。
- **Key-value**: 一つのresourceの属性には`dl`を使う。
- **Disclosure**: secondary metadataやdiagnosticは具体的なlabelを持つ`details`などへ置く。

Design-labではcomponent名と実際のsampleだけを表示する。規則とrationaleをUIへ書かない。

# Workspace Web Design Language

この文書は、Workspace Webのrendered resultが満たす設計規則を定義する。Product固有のresource構成は[`product-ux.md`](product-ux.md)、application shellとSidebar slotの実装構造は[`application-architecture.md`](application-architecture.md)、CSSとsource配置は[`../../../web/workspace/README.md`](../../../web/workspace/README.md)、visual review手順は[`visual-review.md`](visual-review.md)をauthorityとする。

一つの規則は最も具体的な一箇所だけをauthorityとする。具体的な二案の一方を棄却できない文は規則として残さない。

## 1. Principles

複数の規則や要求が競合した場合だけ、この章の優先原理を使う。

### Task before explanation

次の有効なactionへ到達する前の操作、scroll、常設説明が少ない案を選ぶ。ただし、説明を省くと対象、結果、permission、不可逆性を誤認する場合は説明をactionより前に置く。

### Meaning before decoration

情報の関係、重要度、操作可能性、stateのいずれも変えない視覚差を追加しない。

### Authority before convenience

Backend authorityが確定していないdata、state、permission、actionを、UIの都合で補完または推測する案を棄却する。

## 2. Semantics / Grammar

### Information structure

- pageと、page内で一つのtaskまたは判断を完結させる領域をscopeとする。
- 各scopeで最も強い視覚要素はheading、現在判断が必要なattention、filled actionのいずれか一つだけにする。同じ強さのheading、attention、filled actionを複数置かない。
- hierarchyをcolorだけで表現しない。DOMと画面上の配置順を一致させた上で、size、weight、contrastを使う。
- scopeを識別するprimary titleは、一つのscopeにつき一度だけ表示する。
- actionは、そのactionが影響するscope内に置く。
- DTOにfieldが存在するという理由だけで表示せず、current taskの判断またはactionを変える値だけを常設する。

**Disclosure**

- Raw dataやデバッグ情報など、通常は必要としない詳細な情報は折りたたみ領域に隠す。
- 作成または編集のformを表示用screenへ常時同居させず、明示的なactionで開く折りたたみ領域または別pageに置く。

### Interaction and state

- actionにはbutton、navigationにはlink、選択には対応するform controlを使う。
- interactive elementはhover、focus、active、disabledを視覚的に区別する。
- staticなstatus、key、metadataをbuttonやpillの形にしない。
- clickable rowは遷移先を一つだけ持ち、row全体で同じ遷移を実行する。
- permissionがないactionをdisabled controlとして表示しない。

**State and operation**

- stateの違いによってユーザーの判断または利用できるactionが変わる場合、その違いを一つの表示へ統合しない。
- UI stateはそのstateを所有するauthorityから取得し、別domainやtransportのsignalから推測しない。
- 非同期mutationでは楽観的更新か悲観的更新を選ぶ。楽観的更新は暫定stateを明示し、failure時のrollbackまたは再取得を定義できる場合だけ使う。それ以外は確定済みstateを維持し、operationのpending stateを別に表示する。
- loading、failure、unavailableの表示は利用不能になった最小のscopeだけを置き換え、利用可能な親scopeとsibling scopeを残す。
- destructive actionは対象と結果を明記し、復元できない場合だけconfirmationを要求する。

### Language / Copy

- title、navigation label、control labelで明らかな内容を説明文で繰り返さない。
- 常設proseは、対象またはactionの選択を変えるwarning、permission、validation constraint、error recoveryに必要な内容だけにする。
- button labelには具体的なactionを表す動詞を使い、`Submit`と`OK`を使わない。
- identifier、API名、type名を別の語へ言い換えない。

### Responsive behavior

- page全体にhorizontal scrollを発生させない。
- wrappingまたはstack化によって情報の対応関係が失われる場合だけ、その領域内にbounded horizontal scrollを持たせる。それ以外のside-by-side layoutは一列にする。
- primary actionを初期viewportから追い出さない。
- contentをtruncateする場合、完全な値をcopyまたはdetailで取得できるようにする。

### Accessibility

- action、navigation、form controlを非semantic elementだけで実装しない。
- すべてのcontrolにaccessible nameを与える。
- keyboard focusを不可視にしない。
- tab orderを画面上の順序と一致させる。
- stateをcolorだけで区別しない。
- horizontal scroll regionをkeyboard focus可能にし、accessible nameを与える。

## 3. Primitives / Tokens

### Color / Theme

colorはCSS custom propertyをauthorityとし、通常のWorkspace colorはOKLCHで定義する。lightとdarkは同じsemantic token名を使う。

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

- `--bg`、`--bg-raised`、`--bg-subtle`へhueを持たせない。
- accentはfocus、current navigation、current selectionだけに使い、通常本文や通常actionの背景へ使わない。
- success、warning、dangerを、それぞれ対応するstate以外へ使わない。
- componentへraw colorを追加せず、上記semantic tokenを使う。
- ConsoleとterminalのANSI paletteを通常のstatusやformへ流用しない。

### Typography

font sizeは、その領域でユーザーが行う読み方によって選ぶ。

- defaultは`14px / 20px`とし、個々のtextを読むこと自体がtaskの中心になる領域に使う。
- 同じ構造の反復を一覧として走査する領域と、位置、順序、形、選択状態を中心に識別するUI chromeには`12px / 16px`を使う。textを読まなければactionの意味や結果を判断できない場合は`14px / 20px`を使う。
- `24px / 32px`はページまたはdocument内で唯一かつ最大のheadingだけに使う。
- 収めるため、または情報を弱く見せるためにfont sizeを下げない。`12px`未満と`13px`を使わない。
- 機械的な値にはmonospace familyを使う。

```css
--font-sans
--font-mono
--font-size-title /* 24px */
--line-height-title /* 32px */
--font-size-body /* 14px */
--line-height-body /* 20px */
--font-size-compact /* 12px */
--line-height-compact /* 16px */
```

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

- 一つの境界にspacing、line、surface backgroundを重ねず、primary cueを一つだけ使う。
- 汎用的な`Card` primitiveを設けない。
- nested surfaceでparentと同じ境界表現を繰り返さない。
- shadowは通常flowから浮くmenu、popover、Tooltipだけに使う。
- radiusはinteractive controlだけに使う。

### Motion

- motion完了をactionの受付、state change、content理解の条件にしない。
- 繰り返しまたは連続animationを、`prefers-reduced-motion: reduce`で停止する。

## 4. Components / Patterns

### Border / Structural edge

- `Bevel`は1pxの単色borderを描くvisual wrapperであり、照明、raised／inset、ridge／grooveを表現しない。
- `Bevel`自体へbackgroundやsurface toneを与えず、必要なbackgroundは内側のsemantic childが持つtext/content areaへ適用する。
- 隣接するsurfaceでは、実際に境界となる辺だけを有効にする。
- standaloneな閉じた領域は全辺を`Bevel`で囲う。Desktop shellではHeaderのbottom edgeとSidebarのright edgeだけを有効にし、viewport外周のtop／left edgeを描かない。
- `BevelLine`は開いた領域内の1px separatorだけに使う。同じlayerではheading、本文、Lineの端を揃え、Lineだけに端方向のpaddingやmarginを追加しない。
- nested layerでは親layoutがそのlayer全体へinline方向の余白を与え、content、row、Lineを一緒に移動する。Lineだけを短くしない。
- control自身のborder、focus ring、status marker、table gridを`Bevel`または`BevelLine`で置き換えない。

### Sidebar

- Desktop幅は`clamp(220px, 20vw, 280px)`とする。
- navigation contentだけをscrollさせる。
- Desktopのfold controlはSidebar下部、Mobileのfold controlはHeaderに置く。
- MobileでSidebarを表示するときはHeader下の残りviewport全体を使い、main contentと同時表示しない。
- navigationとscope control以外を置かない。
- navigation linkはSidebar幅全体を使うflat rowとする。
- active routeは一つの視覚cueと`aria-current="page"`で示す。
- iconだけでnavigation labelを置き換えない。
- fold controlは一つだけ置き、current stateに対応するaccessible nameを使う。

### Tooltip / Contextual help

- operationやtable全体を説明する常設proseを置く前に、button labelとcolumn headingだけで識別できるようにする。
- labelだけでは表現できない補助説明だけを、対象へbindしたTooltipに入れる。
- pointer hoverとkeyboard focusの両方で開く。
- triggerとTooltipを`aria-describedby`で関連付ける。
- hoverはdelay後、focusは直ちに表示する。
- `Escape`で閉じ、triggerからfocusを移動しない。
- 複数paragraph、form、link、buttonを入れない。
- error、permission、validation constraint、不可逆操作の結果、current stateをTooltipへ入れない。
- touch環境ではTooltipだけを説明への到達手段にしない。

### Generic component patterns

- **Form field**: label、control、constraintまたはhelp、field errorの順に配置する。
- **Key-value**: 一つのresourceの属性には`dl`を使う。

Design-labではcomponent名と実際のsampleだけを表示する。規則とrationaleをUIへ書かない。

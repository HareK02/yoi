# Workspace Web Application Architecture

この文書は、Workspace Webのapplication shell、Header、Sidebar、nested
layoutのcomposition authorityを定義する。Navigation
IAは[`product-ux.md`](product-ux.md)、視覚とinteractionの共通規則は[`design-language.md`](design-language.md)、具体的なsource
pathとCSS
ownershipは[`../../../web/workspace/README.md`](../../../web/workspace/README.md)を参照する。

## Application shell

root layoutが唯一のapplication shellを所有する。

- Sidebar frame
- global fallback navigation
- Header frame
- main content
- SidebarとHeaderのoverride context

page routeはapplication shell、global Header、Sidebar、fold
controlを再実装しない。Pageのmarkupは標準shellのmainへ配置するcontentから始める。

## Header composition

Headerはroot shellが一度だけrenderする。Nested
layoutは`HeaderOverride`を通じて現在scopeのlocation contentを登録する。

- page componentはHeader frameを作らない。
- 最も深いactive overrideを表示する。
- layoutが破棄されたら直前のoverrideへ戻る。
- overrideがなければroot fallbackを表示する。
- route titleをmain contentへ複製しない。

## Sidebar composition

### Root

root shellは`SidebarFrame`と`GlobalSidebar`を一度だけrenderし、root
`SidebarController`を提供する。Overrideがない場合、`GlobalNavSections`がfallbackになる。

### Workspace

Workspace layoutは、親のroot
slotへ`WorkspaceSidebar`を登録する。同時に、さらに深いlayoutが使う新しい`SidebarController`をcontextへ設定する。

`WorkspaceSidebar`はWorkspace shortcut headerを保持し、その下へchild
slotをrenderする。Child overrideがない場合だけ、Workspace navigation
fallbackを表示する。

### Settings

Settings layoutは、Workspace layoutが提供したchild
slotへ`SettingsSidebar`を登録する。Workspace shortcut headerは残り、Workspace
navigation fallbackだけがSettings navigationへ置き換わる。

さらに深いscopeも同じcontractを使う。

## Override stack

`HeaderOverride`と`SidebarOverride`は、layout lifecycleにbindしたLIFO
stackとして扱う。

1. Layout mount時にparent controllerへregisterする。
2. Controllerは最も新しいactive registrationを表示する。
3. Nested layoutはchild controllerを新しく提供する。
4. Layout destroy時に自分のregistrationをremoveする。
5. 一つ前のregistrationがあれば復元する。
6. Stackが空ならfallbackへ戻る。

同じregistrationを複数回removeしても結果を変えない。古いcleanupが新しいactive
registrationを削除してはならない。

## Ownership

- `SidebarFrame`だけが`aside`、外枠、scroll領域、fold controlを所有する。
- `GlobalSidebar`、`WorkspaceSidebar`、`SettingsSidebar`は各scopeのcontentを所有する。
- `SidebarOverride`を登録できるのはscopeを定義するlayoutだけとする。
- page component、一時的なwidget、dialogはSidebarを差し替えない。
- child layoutは親navigationをcopyせず、自分のsidebar layerだけをregisterする。
- Global、Workspace、Settingsを一つのsnippetへ平坦化しない。

## Data authority

Sidebar compositionはnavigation
structureを決めるが、Workspace、permission、Worker
stateのauthorityにはならない。

- Workspace identityとpermissionはBackend projectionを使う。
- Worker listとstateはWorkspace protocol projectionを使う。
- route pathやdisplay labelからauthorityを推測しない。
- unavailable dataを架空の正常stateで置き換えない。
- permissionにより利用できないnavigationは、権威あるpermission
  projectionに基づいて除外する。

## Design-lab

認証不要のstatic design-labはroot shellを再利用し、実際と同じnested
controllerとoverride stackを構成する。

Live Workspace protocolへbindするresource listだけは、同じscope layer内のstatic
fixtureへ置き換えてよい。Static fixtureへ別scopeのnavigationを混ぜず、production
componentと同じ順序、state表現を使う。

Design-lab固有のswitch controlでSidebarを差し替えず、route
hierarchyによってlayout lifecycleを発生させる。

# BKSS Core Foundation 仕様

この文書は BKSS の初期 core + UI foundation に適用する規範的な挙動と受け入れ条件を定める。BKSS は Rust/GPUI による Hyprland 向けシェル構築ライブラリで、ガラスの背景効果は Hyprglass が担当する。ここにある型名やコード例は概念を示すもので、公開 API の確定シグネチャではない。

数値で「初期値」「default」と記した箇所は調整可能な暫定値であり、特に視覚値は実行・視覚確認で調整してよい。公開される挙動と受け入れ条件は、API 名や内部実装の未確定を理由に変更しない。GPUI/Wayland の実現性確認と実装状況は開発者向け資料に分離する。

Overlay の keyboard 契約は single Wayland seat に限定し、OS/compositor focus の制御や復帰は保証しない。

## 範囲と境界

`bkss-core` は Glass、runtime、reactive/resource、低レベルの surface と lifecycle 機構を提供する。`bkss-ui` は常設の上位 UI package として、公開 animation API と BottomBar/Overlay などの再利用可能な汎用 widget を提供し、core に依存する。core から ui への依存や循環依存は作らない。外部 integration は core の trait や source に接続する別 package とし、core は Time や Hyprland IPC に依存しない。GPUI の custom elements/components と便利な BKSS API の併用を許す。

## runtime 契約

- runtime が update、subscription、redraw、animation の実行・スケジューリング、surface の cleanup を所有する。公開 animation 定義 API は ui に置き、ui の widget/surface wrapper が opacity の実行記述を core runtime に渡し、計算済みの sample を描画へ適用する。
- resource は runtime のスコープで共有し、同じ resource を複数 active UI consumer が利用できる。source は最初の consumer で lazy に開始し、source handle の clone だけでは active consumer と数えない。
- 最後の consumer が終了すると subscription/resource は停止する。exit animation 中の widget は unmount まで mounted なので購読を保持し、unmount 後に他の consumer がなければ停止する。次の activation は resource を再取得・再接続し、初期状態を得る。resource identity は source type + normalized configuration key で区別する。公開型や内部実装は未確定である。
- 停止した activation から遅れて届く更新は activation identity で破棄し、再取得後の状態を壊さない。
- test source は実際の Time integration を使わず、更新、unsubscribe、共有、cleanup を決定的に検証できる。
- GPUI の event/notification、window、paint、task 等を優先して再利用する。独自の依存グラフや scheduler は、既存の機構で要件を満たせない場合に限って検討する。

### 採用する技術方針

- UI-local な `State<T>` は初期値を ready として runtime の setup/mount で生成する。clone した State handle は同じ cell を共有し、どの handle からの明示的な更新も同じ State を更新する。read-only の `Reactive<T>` を公開し、source の `Loading/Ready` と混同しない。
- 別 thread からの publisher は `Send` payload を受け、UI thread へ渡して更新する。更新関数は純粋かつ同期的にする。初期 scope は `map` と binary `combine` のみで、async map/filter は追加しない。
- source は `SourceState<T>` のような `Loading/Ready` wrapper で表す。`map` は loading を引き継ぎ、`combine` は全入力が ready になるまで loading、その後は各更新時点の最新値を合成する。T が `Option` の場合の `None` は domain value である。
- 明示された State publication は毎回 dependent を更新し、等値による抑制はしない。UI の redraw は自然にまとめられてよい。複数 State の atomic transaction は約束せず、独自 graph も必要性が実証されるまで作らない。
- pre-run の builder/source は inert descriptor とし、runtime-bound な State/entity は setup/mount で生成する。source handle の clone は consumer ではなく、mounted な binding の読み取り lease が consumer である。setup で作った State を callback が capture することは許す。
- resource identity は source type + normalized configuration key とし、異なる設定の接続を分離しながら runtime 全体で共有する。停止した activation から遅れて届く更新は activation identity で破棄する。
- ui の Animation は小さな opacity 実行記述へ変換し、core に汎用 property engine や過剰な trait 階層を導入しない。spring は初期 scope に含まれる必須機能とし、native 実装が BKSS 仕様と合わない場合は根拠を得た上で最小の runner を検討する。

## integration failure と終了

backend の接続または subscription の failure は Shell 全体の fatal error とする。単一 backend の故障でも全 surface を閉じる。「Bluetooth device が未接続」「MPRIS の再生がない」「一覧が空」は有効な domain state であり、failure ではない。Shell の実行は概念上 `Result<(), ShellError>` を返し、library 自身は `process::exit` せず、呼び出し側が exit code を決める。`ShellError` は原因と context を持つが、最終 struct は GPUI 依存の検証後に決める。fatal shutdown では decorative exit animation の完了を待たず、runtime が subscription、task、shared resource、surface の cleanup を best-effort で行う。ただし外部 failure 中の cleanup 成功は保証しない。fatal 後の自動 reconnect は行わない。

## UI と surface

Glass と低レベルの surface/lifecycle は core に置く。BottomBar、Overlay などの汎用 widget と公開 animation API は ui に置き、ui が core の機構を利用する。core の Glass builder 自体が ui の Animation に依存する設計にはしない。

Shell の runtime entry point は core に置き、汎用の低レベル surface descriptor を受け取る。`.bottom_bar(...)` や `.overlay(...)` の宣言的な便利 API は、ui の拡張 API（`ShellExt`/prelude は名称案）として提供し、core から ui へ依存しない。初期の Shell/State binding は runtime 上でのみ有効とし、直接 GPUI と組み合わせる setup の逃げ道は残すが、context の具体型や呼び出し方法は未確定である。

### BottomBar（ui widget）

静的 Glass を表示する layer-shell surface。logical height 分を exclusive zone に予約し、keyboard focus は要求しない。display ごとの選択と bounds を扱う。複数 display は起動時に接続済みの全対象を同時に受け入れる。

**surface 取得の遅延**: Shell は外部 source の初回取得を待たずに表示できる一方、layer-shell surface は必要な display が列挙されてから作成する。display 準備前に surface 作成を試みず、準備後に一度だけ作成する。

### Overlay（ui widget）

`OverlayDefinition` は inert な設定で、display ごとの fallible content factory は mount 時に一度実行する。公開 API は `OverlayRuntimeHandleExt::mount_overlay(definition, factory, cx)` と、display 指定の `OverlayHandle::open(display, cx)` / `close(display, cx)` である。外部 handle は runtime を延命しない。core runtime は動的 registration、display readiness、generation、pending/mounted surface の所有・cleanup を担い、ui は definition、display-local state、rendering を担う。mounted content が resource binding/lease を所有し、同じ runtime scope 内の resource sharing と最後の lease での停止は既存契約に従う。

同じ definition でも core runtime が display ごとの lifecycle と visibility を独立に管理し、各 display の content factory を mount 時に実行する。`KeyboardFocus::None | Interactive` は definition 作成時に明示する。Interactive は surface 作成時の `OnDemand` を要求し、None は `None` で作成する。初期契約は single seat であり、BKSS は overlay 間の keyboard arbitration、seat ごとの ordering、native activation/focus、focus restoration を行わない。compositor が実際の focus と同一 layer 内の visual order を決める。ユーザー click は通常の入力であり、redundant open は no-op で surface を raise しない。

Escape は実際に Escape event を受け取った mounted `Interactive` instance だけを閉じる。None instance への Escape 配達は約束しない。外部 handle、Escape、backdrop の close は同じ lifecycle 経路を使う。animation 未指定時と exit animation なしの場合は即時に閉じ、exit animation 指定時は完了までその display instance の surface・content・resource lease を保持する。

open は display ごとに `Closed | Pending | Mounted` lifecycle を持つ。repeated open は pending/mounted instance を activate、raise、recreate しない。exit 中の reopen は同じ mount generation・window・content を保持し、現在値と速度から enter へ retarget する。旧 exit completion は無効化する。pending close は factory を呼ばず取消し、完全に閉じた後の genuine open は新 generation で開始する。stale factory result はその正確な旧 window だけを回収し、callback 後に generation/runtime を再確認する。対象 instance close はその view/surface と lease のみ解放し、他 display や runtime の別 surface を閉じない。runtime shutdown/fatal は animation を即時取消し、pending を含む全 runtime-owned surface を cleanup し、fatal primary cause と attach host の所有権を維持する。display ごとの visibility/content teardown と runtime-wide resource sharing は独立に検証する。

## Geometry / Input API

### BottomBar

BottomBar は display 下端に anchor する常設 layer-shell surface である。視覚的な Glass 領域と layer surface の allocation 領域は分離して考える。

- **Width policy（可逆的 default）**
  - `FullDisplayWidth`: Glass visual width を display 幅いっぱいに伸ばす。
  - `FloatingContentWidth`: 内容幅に合わせた Glass visual width。layer surface 自体は display 全幅を取る可能性があるが、Glass の描画領域は content width + padding までに制限する。
  - 初期 default は `FloatingContentWidth` とする。

- **Margins / alignment**
  - 下端 anchor は固定。optional な logical margins（left, right, bottom）を持つ。margin は視覚的な Glass の位置を決めると同時に、reserve 計算にも使う。
  - `FloatingContentWidth` では horizontal alignment を `Center | Left | Right` とし、margin と合わせて配置する。

- **Reservation**
  - `reserve` は default ON。OFF にすると exclusive zone を 0 にし、アプリケーションウィンドウと重ねる。
  - 予約高さは「bar の logical height + bottom margin」のみとし、shadow は含めない。layer surface の実ピクセル高さは shadow 込みでも、compositor への exclusive zone 報告が shadow を重複計算しないよう注意する。

- **Display bounds / scale**
  - geometry は logical units で宣言する。display scale は platform 適用後のピクセル計算に反映する。複数 display では各 display の bounds/scale を独立に扱う。

- **Pointer hit area（実現性注意）**
  - 理想的にはポインタヒット領域は visual Glass 領域に限定したい。
  - `FloatingContentWidth` で layer surface が display 全幅のままの場合、透明な allocation 領域がクリックを横取りする可能性がある。visual 領域に限定した input region を優先し、実現性がない場合の fallback は利用者が承認した条件付き選択に従う。不可視入力層でクリック妨害を回避してはならない。

### Overlay

- **Placement**
  - default は現在の configured local viewport の中央。
  - explicit anchor + offset も最小 API で提供する。anchor は `Top | Bottom | Left | Right | Center` の組み合わせ、offset は logical units。
  - native request は display ごとの独立 layer-shell surface、四辺 anchor、requested size `0 × 0`、`exclusive_zone = 0` とする。配置・clamp・clip の基準は実際に configure された window-local viewport と display scale であり、global usable bounds/origin は利用可能と仮定しない。
- **Oversized content**
  - viewport を超える content はその configured local viewport 内で clip する。利用者は明示的に scroll container を選べる。自動的な文字サイズ縮小（typography scaling）や賢い reposition は行わない。
- **Scale**
  - 各 display の scale に応じて logical size を pixel size に変換し、clamp は pixel 換算後の configured viewport bounds で行う。
- **Reservation / BottomBar relation**
  - Overlay は exclusive zone を予約しない。予約付き BottomBar を避ける配置が必要な場合は、Overlay を開く前に bar の初期 allocation/reservation が安定していることを要求する。
  - compositor の layer order は物理的に BottomBar の背後へ積む保証ではない。同一 layer の順序は compositor が決める。非予約 bar は Overlay に隠れたり入力を妨げられたりし得る。予約の後続変化に対する adaptive relayout/resizing は保証しない。実際に viewport resize event を受けた場合は、その時点の viewport に追従する。

### Input modes

- **Keyboard mode**
  - `None`: surface creation mode `None` を使い、keyboard input を要求しない。keyboard 配達・Escape は約束しない。pointer click-through の指定でもない。
  - `Interactive`: surface creation mode `OnDemand` を使う。これは BKSS の keyboard arbitration や OS activation/focus 成功保証ではない。実際の focus は compositor が決め、focus 取得・復帰を理由とする surface 再作成はしない。

- **Outside-close backdrop（pointer interactive overlay）**
  - default OFF。opt-in のみ。
  - ON の場合、backdrop は content と同じ root/surface に描画し、入力領域は configured local viewport 内で content の外側に限定する。そこへの pointer down は close を発行して消費し、背後のアプリケーションには転送しない。display 外側や local viewport 外側を捕捉しない。
  - backdrop は別 surface/focus owner ではなく、独立した keyboard focus/arbitration を持たない。
  - default OFF の場合は backdrop paint/input を作らず、input region は content のみ。透明な full-display input layer は作らない。

- **Stacking**
  - backdrop と content は同じ root/surface で content 外側を backdrop paint/input とする。別 surface の物理 stacking、BottomBar より背後の配置、同一 layer order は保証しない。input region は上述の local viewport に限定する。

### 入力配達の受け入れ

pointer / keyboard の入力配達は CF-17 と CF-18 に従う。検証が必要な実現性・backend 固有の事項は、公開仕様ではなく実装者向け確認事項として記録する。

## 公開 API 草案（概念名・未コンパイル）

```text
State<T>                   // setup/mount で生成、clone は同じ cell を共有
Reactive<T>                // State の read-only view
Reactive<A>.map(f) -> Reactive<B>
Reactive<A>.combine(Reactive<B>, f) -> Reactive<C>
SourceState<T>              // 取得前 / 取得済みの値を表す状態
SourceDescriptor<T>         // 宣言時には接続せず、利用開始時に接続
OverlayHandle::open(display, cx) -> Result<(), SurfaceRequestError>
OverlayHandle::close(display, cx) -> Result<(), SurfaceRequestError>
OverlayDefinition::new(keyboard: KeyboardFocus)
OverlayRuntimeHandleExt::mount_overlay(definition, factory, cx) -> Result<OverlayHandle, ShellError>
Shell::run(...) -> Result<(), ShellError>

BottomBarWidth::FullDisplayWidth
BottomBarWidth::FloatingContentWidth { max_logical_width, horizontal_alignment }
BottomBarGeometry { width, logical_height, margins, reserve }
OverlayPlacement::Center
OverlayPlacement::Anchor { edges, offset }
OverlayGeometry { placement, max_logical_size, oversized }
KeyboardFocus::None | KeyboardFocus::Interactive
PointerDismiss::Disabled | PointerDismiss::Backdrop  // consumes click

GlassStyle { radius, opacity, tint, border, rim, inner_highlight, inner_shadow }
GlassStyle::with_hover(partial) -> GlassStyle
GlassStyle::with_pressed(partial) -> GlassStyle
GlassStyle::with_focus(partial) -> GlassStyle

enum Timing {
    Easing { duration_ms: u64, curve: EasingCurve },
    Spring { stiffness: f32, damping: f32, mass: f32, max_duration_ms: u64 },
}

Animation::enter(Timing)
Animation::exit(Timing)
Animation::transition(Timing)  // enter と逆 exit
```

概念的には inert な builder と source descriptor を先に作り、setup/mount で `State` と entity を runtime に束縛する。それを複数 display の独立 widget instance に渡し、State handle を clone して同じ cell を共有する。GPUI component composition はこの API に限定せず、そのまま利用できる。global に definition を mutable singleton として置く設計は採らない。上の例は説明用で、コンパイル可能性を示すものではない。

## reactive binding

静的 binding と source 由来の reactive binding を区別する。初期に必要な操作の意味は `map`（値の変換）と `combine`（複数値の合成）である。購読解除は runtime の所有する lifecycle に従う。filter の初期値・拒否値・再通知の意味は未決定なので、API に追加しない。

外部 source の初回取得を待たずに Shell を表示する。source は「取得前」と「取得済み」を区別して公開し、利用者は取得前の代替表示を宣言できる。取得済みの空一覧や未接続状態を、取得前の代用にはしない。接続・取得に失敗した場合は、前述の Shell 全体終了方針に従う。resource を停止して再取得する場合も、古い値を最新値として扱わず、取得前から開始する。

`map` は取得済みの値にだけ純粋な変換を適用し、取得前の状態を引き継ぐ。`combine` は必要な入力がすべて取得済みになるまで取得前とし、以後はいずれかの入力が更新されるたびに最新値を合成する。明示的な publication は同値でも通知し、複数 State の同時更新は atomic ではない。

## Glass

Glass は純粋な視覚プリミティブである。`GlassStyle` は以下の描画指定を持つ。

- `radius`: 角丸半径（logical units）
- `opacity`: 全体の不透明度（alpha multiplier）
- `tint`: ベース色（独自のアルファを持ちうる）
- optional `border` / `rim`
- optional `inner_highlight` / `inner_shadow`

最終的な描画色は `tint` のアルファに `opacity` を乗算する。つまり `rendered_alpha = tint.alpha * opacity`。tint アルファと opacity の二重定義を防ぐため、両方を同時に「alpha を重ねる」と解釈し、一方を無視したり clamp したりしない。

`GlassStyle.opacity` は Glass 装飾自体（tint 背景、border、rim、highlights/shadows）に適用される alpha multiplier であり、Glass の子 content や親 container のグローバル不透明度ではない。content の不透明度は GPUI の通常の描画ルールに従う。content に対して opacity animation をかけたい場合は、Glass wrapper に対して別途 ui animation を適用する。

### 初期値

デフォルトは過激な OS テーマ連携なしで、中性かつ利用可能な値とする。以下の視覚初期値は設定可能である。

- tint: rgba(0, 0, 0, 0.15)
- opacity: 1.0
- radius: 12 logical px
- border / rim / inner_highlight / inner_shadow: 未指定なら off

### インタラクション状態

hover / pressed / focus などの外観変化は、**ユーザーがその状態用の style を明示的に指定した場合のみ**適用される。Glass 自体は自動的な効果を持たない。

- `GlassStyle` は immutable な clone input とする。状態別 style は base style に対する partial override とし、未指定フィールドは base から継承する。
- 適用順序: `base -> hover -> pressed`。focus に関する装飾は独立して最後に適用し、focus 表示が他の状態によって消えないようにする。
- focus 表示を指定しない場合は focus indicator は表示されないが、それは GPUI の default focus 制御を隠すものではない。
- Glass は装飾プリミティブであり、style があるだけで親要素全体を focusable/clickable にしない。focus/click セマンティクスは、明示的に interactive な wrapper や component が持つ。
- インタラクション状態の変化は animation 未指定時は即時反映する。
- animation 定義は ui に置き、core の Glass は ui Animation に依存しない。ただし ui の animation 定義が core Glass style の数値を変更できる構造は互換とする。

### blur / 背景効果

背景の取得、ぼかし、屈折、色収差、光沢は Hyprglass が compositor 側で担当する。GPUI は Glass の形状、薄い tint、明示的な装飾と子 content を描画する。`GlassStyle` の指定を Hyprglass の shader parameter に自動転送しない。

Hyprglass の導入・ロード・設定・reload は利用者の環境管理の責務とする。library が `hyprctl` を呼び出したり設定ファイルを書き換えたりせず、利用可否の自動検出も行わない。Hyprglass が利用できない場合の背景効果は保証せず、他 compositor 向けの代替ガラス backend は設けない。通常の GPUI 描画だけが残ることを、液体ガラスへの代替表示として扱わない。

標準設定の適用対象は namespace `bkss-bottom-bar` の BottomBar とする。core の layer 生成は `LayerSurfaceConfig.options.namespace` を保持し、Overlay の `bkss-overlay` と分離する。`SurfaceDescriptor` の固定 namespace `bkss-core-surface` は変更しない。

同一 surface 内の widget 間屈折は対象外とする。Overlay の backdrop と opacity animation は alpha による輪郭推定と干渉するため、今回の設定例には含めない。既存の Overlay の入力・dismiss・animation の契約は維持する。設定と適用上の制約は [Hyprglass 連携](hyprglass.md)を参照。

## animation

公開 animation API は ui に置く。アニメーションは opt-in の純粋な定義として enter、exit、transition に attach する。対象 property は opacity、補間は linear/easing/spring とする。実行、lifecycle との接続、スケジューリングは core runtime が担う。transition は enter 定義と、その逆向きの exit 定義を設定する。同じ target への後続 builder assignment が勝つ。デフォルトではアニメーションしない。途中で再 open した場合は現在値から新しい target へ進み、endpoint へ jump しない。

### timing kind

`Timing` は `Easing` と `Spring` を mutually exclusive にする。spring に duration を与えても固定 duration として動作させない。

```text
enum Timing {
    Easing { duration_ms: u64, curve: EasingCurve },
    Spring { stiffness: f32, damping: f32, mass: f32, max_duration_ms: u64 },
}
```

### easing

- default duration: 160ms ease-out。
- duration は >= 0 の有限値。0 は即時。
- easing 時は指定した duration を再適用する。

### spring（minimal physical model）

spring は damped harmonic oscillator の物理パラメータを公開する。これは GPUI native 実装の主張ではなく、BKSS ui レイヤーの仕様である。

- パラメータ: `stiffness > 0`, `damping > 0`（finite）, `mass > 0`, `max_duration_ms > 0`（finite）。
- 初期値: `mass = 1.0`, `stiffness = 300.0`, `damping = 30.0`, `max_duration_ms = 2000`。調整可能な仮値である。
- retarget 時は可能な限り現在値と速度を維持し、endpoint へ jump しない。redundant open は max duration をリセットしない。
- 内部 solver の速度は、描画に使う clamp によって汚染しない。render 値だけを clamp する。
- opacity の endpoint は [0, 1] とし、spring 内部で overshoot しても render opacity は [0, 1] に clamp する。
- settle 判定: 正規化された位置誤差と速度の両方が eps = 0.001 未満になったとき completed とする。位置誤差の正規化は固定の非ゼロ channel reference span で行う。採用する正規化 span は `max(abs(original transition endpoint span), 1.0)` で、span が 0 や retarget 後でも tolerance が変わらず、0 除算も起きない。速度は「単位 / 秒」で正規化する。
- 位置誤差が eps 未満で速度がほぼ 0 の微小静止誤差の場合、endpoint に 1 回だけ snap して completed とする。
- 開始時に距離 0 でも速度が 0 でない場合は即時 completed とはせず、速度が収束するまで実行する。
- max elapsed 安全 bound: genuine な新規 request あたり default 2000ms。到達したら endpoint に snap し、completion callback をちょうど 1 回だけ発行する。cancel/stale された completion は新しい表示を破棄しない。
- 時間は monotonic clock を使い、FPS 依存にしない。frame rate 独立性のテストでは、30/60/144Hz の離散的な模擬 clock を同一 elapsed 時刻でサンプリングし、宣言された数値 tolerance 内で軌道と completion が一致することを検証する。bit-identical 一致を要求しない。
- 数値初期値は仕様上の仮値であり、実装・視覚検証で調整してよい。

## 要件と受け入れ条件

| ID | 要件 | 受け入れ条件 |
|---|---|---|
| CF-01 | core/ui 境界 | core + ui の foundation が特定 integration なしで build/API 検証でき、ui が core に依存し core から ui へ依存しない |
| CF-02 | Glass | GPUI の tint・明示装飾・子 content の契約を維持し、設定済み Hyprglass により BottomBar の背後の画面へ背景効果を適用できる |
| CF-03 | BottomBar | 起動時の複数 display に layer-shell surface、logical-height の予約、非フォーカスを確認できる |
| CF-04 | Overlay | display 別 open/close、独立 visibility/content/lifecycle、definition 時の keyboard mode 選択、実際に配達された Escape による Interactive instance close を確認できる |
| CF-05 | lifecycle | exit 完了前に破棄せず、exit なしは即時 close、反復要求、現在値からの retarget、stale completion 防止をテストできる |
| CF-06 | animation | opt-in の opacity を fake clock で検証し、未指定時は即時状態になる |
| CF-07 | reactive | test source の update、unsubscribe、resource sharing、cleanup を検証でき、最後の consumer 後に停止し次回利用で再取得する |
| CF-08 | displays | 起動時に接続済み複数 display を同時表示できる。hotplug は対象外と明記する |
| CF-09 | fatal integration failure | acquisition/subscription の開始時・開始後 failure が原因付きで伝播し、Shell 全体を終了する。cleanup は重複せず、shutdown 中に別 backend を開始しない |
| CF-10 | 初回取得と代替表示 | 遅延する test source でも Shell が表示され、取得前の代替表示から取得済みの表示へ更新される。空一覧との区別と再取得時の状態も検証できる |
| CF-11 | State/Reactive | State write、read-only Reactive、loading→ready、map/combine propagation を検証できる |
| CF-12 | display / single-seat keyboard | 二 display の独立 visibility/content teardown、共有 resource、None/Interactive creation mode と actual delivered-key recipient を検証できる。BKSS seat ordering/arbitration、native activation/focus restoration は保証しない |
| CF-13 | State/activation | ready 初期値、clone handle による同一 cell 更新、停止後 activation の stale update 破棄を検証できる |
| CF-14 | Shell 終了 | fatal cause/context を `Result<(), ShellError>` で返し、library が process exit せず、cleanup 中に decorative animation を待たないことを検証できる |
| CF-15 | BottomBar geometry | 二つの monitor size / 異なる logical scale で、reserve ON/OFF、FullDisplayWidth/FloatingContentWidth、logical margins、bottom anchor、shadow を exclusive zone に含めないことを検証できる |
| CF-16 | Overlay geometry | configured local viewport 中央 default、explicit anchor/offset、configured local viewport/scale による pixel-clamp と clip、明示的 scroll container、自動 shrink/typography scaling なしを検証できる。global usable bounds は前提にしない |
| CF-17 | Input modes | None/Interactive creation semantics と actual key delivery を区別する。backdrop opt-in/default OFF、ON 時 local viewport 内 content-exterior click を消費し app に転送せず、OFF 時 input region が content のみであることを検証できる。native focus/activation、outside-viewport capture、BottomBar behind-order は保証しない。既存 BottomBar の visual Glass hit area intent と、透明 allocation が input を横取りしうる gap は別途記録する |
| CF-18 | 入力配達の検証ゲート | 実装前に GPUI/Wayland の input-region/layer-surface capability と probe evidence を確認し、source/probe だけで native acceptance を主張しない。実装後は actual native example で BottomBar と Overlay の pointer/keyboard delivery を観察・記録する。headless/fake test は native 配達の証拠としない |
| CF-19 | Glass style contract | hover/pressed/focus の外観変化はユーザーが明示指定した場合のみ適用され、Glass 自体は自動効果を持たない。base -> hover -> pressed の順に partial override し、focus は最後に独立適用する。Glass style があるだけで container が focusable/clickable にならない。opacity と tint alpha の乗算ルールを検証できる |
| CF-20 | animation timing | Easing と Spring は mutually exclusive。easing default は 160ms ease-out、duration 0 は即時。spring は stiffness/damping/mass の物理パラメータを持ち、duration を silently override しないことを検証できる |
| CF-21 | spring convergence | settle は正規化位置誤差・速度両方 eps = 0.001 未満、距離 0 かつ速度 0 のときのみ即時 completed、正規化 span は max(|original span|, 1.0)、max elapsed 2000ms per genuine request で endpoint snap + 単一 completion、redundant open は max をリセットせず stale completion は破棄しない、30/60/144Hz で同一 elapsed サンプリングの tolerance 内一致を検証できる |
| CF-22 | invalid params | stiffness <= 0、damping <= 0、mass <= 0、max_duration_ms == 0 など不正な spring パラメータ、および Glass/geometry の不正なパラメータは、panic や無言の sanitize ではなく error で拒否し、副作用を起こす前に検出する。easing duration は u64 なので負は表現不可、duration = 0 は許可 |
| CF-23 | Glass reactive / immutable | `GlassStyle` は immutable clone input とし、reactive な個別プロパティ変更で再描画できる。ui Animation 未指定時は状態変化が即時反映することを検証できる |

## 対象外

- display hotplug。初期要件は起動時に接続済みの display のみ。
- 他 compositor 向けの代替ガラス backend、同一 surface 内の widget 間屈折、Hyprglass の自動導入・ロード・設定検出。
- scale animation、および任意 subtree の入力領域、selection、caret/IME の変換。
- 特定 integration を core に組み込むこと、完成済み shell、未測定の性能保証。

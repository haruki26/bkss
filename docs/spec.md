# BKSS Foundation / BKSS-UI v0.1 仕様

この文書は BKSS の初期 core + UI foundation に適用する規範的な挙動と受け入れ条件を定める。BKSS は Rust/GPUI による Hyprland 向けシェル構築ライブラリで、ガラスの背景効果は Hyprglass が担当する。ここにある型名やコード例は概念を示すもので、公開 API の確定シグネチャではない。

数値で「初期値」「default」と記した箇所は調整可能な暫定値であり、特に視覚値は実行・視覚確認で調整してよい。公開される挙動と受け入れ条件は、API 名や内部実装の未確定を理由に変更しない。GPUI/Wayland の実現性確認と実装状況は開発者向け資料に分離する。

Overlay の keyboard 契約は single Wayland seat に限定し、OS/compositor focus の制御や復帰は保証しない。

## 範囲と境界

`bkss-core` は Glass、runtime、reactive/resource、低レベルの surface と lifecycle 機構を提供する。`bkss-ui` は core に依存する宣言的な desktop shell SDK とし、公開範囲は Shell、Effects、Animation に限定する。Container、Row、Column、Modal、Tooltip 等の汎用 layout／widget／UI preset は提供しない。core から ui への依存や循環依存は作らない。外部 integration は core の Source 契約に接続する別 package とし、core は特定 integration や Hyprland IPC に依存しない。GPUI の layout・style・描画・子要素合成をそのまま利用する。

## runtime 契約

- runtime が update、subscription、redraw、animation の実行・スケジューリング、surface の cleanup を所有する。公開 Transition 定義 API は ui に置き、ui の Surface wrapper が opacity・logical translation X/Y の実行記述を core runtime に渡し、計算済みの sample を描画へ適用する。
- resource は runtime のスコープで共有し、同じ resource を複数 active UI consumer が利用できる。source は最初の consumer で lazy に開始し、source handle の clone だけでは active consumer と数えない。
- 最後の consumer が終了すると subscription/resource は停止する。exit animation 中の widget は unmount まで mounted なので購読を保持し、unmount 後に他の consumer がなければ停止する。次の activation は resource を再取得・再接続し、初期状態を得る。resource identity は source type + normalized configuration key で区別する。公開型や内部実装は未確定である。
- 停止した activation から遅れて届く更新は activation identity で破棄し、再取得後の状態を壊さない。
- test source は実際の Time integration を使わず、更新、unsubscribe、共有、cleanup を決定的に検証できる。
- event/notification、window、paint、task 等は GPUI の機能を利用する。独自の依存グラフや scheduler は設けない。

### 採用する技術方針

- UI-local な `State<T>` は初期値を ready として runtime の setup/mount で生成する。clone した State handle は同じ cell を共有し、どの handle からの明示的な更新も同じ State を更新する。read-only の `Reactive<T>` を公開し、source の `Loading/Ready` と混同しない。
- 別 thread からの publisher は `Send` payload を受け、UI thread へ渡して更新する。更新関数は純粋かつ同期的にする。初期 scope は `map` と binary `combine` のみで、async map/filter は追加しない。
- source は `SourceState<T>` のような `Loading/Ready` wrapper で表す。`map` は loading を引き継ぎ、`combine` は全入力が ready になるまで loading、その後は各更新時点の最新値を合成する。T が `Option` の場合の `None` は domain value である。
- 明示された State publication は毎回 dependent を更新し、等値による抑制はしない。UI の redraw は自然にまとめられてよい。複数 State の atomic transaction は約束せず、独自 graph も必要性が実証されるまで作らない。
- pre-run の builder/source は inert descriptor とし、runtime-bound な State/entity は setup/mount で生成する。source handle の clone は consumer ではなく、mounted な binding の読み取り lease が consumer である。setup で作った State を callback が capture することは許す。
- resource identity は source type + normalized configuration key とし、異なる設定の接続を分離しながら runtime 全体で共有する。停止した activation から遅れて届く更新は activation identity で破棄する。
- ui の Transition は opacity・translation X/Y に限定した実行記述へ変換する。core runner が補間と spring 計算を実行し、汎用 property engine や別の scheduler は設けない。

## integration failure と終了

backend の接続または subscription の failure は Shell 全体の fatal error とする。単一 backend の故障でも全 surface を閉じる。「Bluetooth device が未接続」「MPRIS の再生がない」「一覧が空」は有効な domain state であり、failure ではない。Shell の実行は概念上 `Result<(), ShellError>` を返し、library 自身は `process::exit` せず、呼び出し側が exit code を決める。`ShellError` は原因と context を持つが、最終 struct は GPUI 依存の検証後に決める。fatal shutdown では decorative exit animation の完了を待たず、runtime が subscription、task、shared resource、surface の cleanup を best-effort で行う。ただし外部 failure 中の cleanup 成功は保証しない。fatal 後の自動 reconnect は行わない。

## UI と surface

Glass の基礎描画と低レベルの Surface/lifecycle は core に置く。ui は shell/effects/animation の公開 API を提供し、core の機構を利用する。core の Glass builder 自体が ui の Transition に依存する設計にはしない。

Shell の runtime entry point は core に置く。ui の ShellSurface／Bar／Overlay は inert な Surface 宣言であり、明示的な runtime 登録時に生成を開始する。通常の GPUI Element を child に置くだけで独立した native Surface を生成しない。宣言は再利用可能な設定と style を保持し、消費される Div／AnyElement の木は保持しない。fallible content factory が display／mount ごとの view を生成し、その render が毎回新しい GPUI content を組み立てる。Styled はその内部 root に適用し、ParentElement による合成は新しい content や Glass element で行う。

### Bar

画面端に配置する layer-shell Surface。上下左右の辺を扱い、既定は下端、Layer::Top、keyboard None とする。厚みは確定 logical px 専用型で指定し、auto／割合指定を型で排除する。厚みは有限かつ1 logical px以上とし、数値は副作用前に検証する。GPUI の h／w は内部 content の style であり、native Surface の厚みと予約量を上書きしない。表示・終了・途中反転は Overlay と同じ core controller を使い、登録時に対象 Output への初回表示を要求する。

**surface 取得の遅延**: Shell は外部 source の初回取得を待たずに表示できる一方、layer-shell surface は必要な display が列挙されてから作成する。display 準備前に surface 作成を試みず、準備後に一度だけ作成する。

### Overlay / ShellSurface

Overlay は登録時には閉じており、display 指定の handle で表示・非表示を要求する。既定は Layer::Overlay、exclusive zone 0、透明背景、keyboard None、Transition なし。Glass は自動的に挿入せず、content 側で明示的に合成する。一般の ShellSurface は Surface 設定と初期表示を明示できる。外部 handle は runtime を延命しない。core runtime は動的 registration、display readiness、generation、pending/mounted Surface の所有・cleanup を担う。mounted content が resource binding/lease を所有し、resource は runtime scope 内で共有され、最後の lease の解放で停止する。

同じ宣言でも core runtime が display ごとの lifecycle と visibility を独立に管理し、各 display の content factory を mount 時に実行する。keyboard interactivity は作成時に設定し、OnDemand は通常の user-managed focus を要求するが自動 focus は保証しない。初期契約は single seat であり、BKSS は Surface 間の keyboard arbitration、seat ごとの ordering、native activation/focus、focus restoration を行わない。compositor が実際の focus と同一 Layer 内の visual order を決める。redundant show は no-op で Surface を raise しない。layer、anchor、namespace、size policy、margin、予約、keyboard mode は登録時に固定し、GPUI style 変更から native 設定を再作成しない。

Escape は実際に event を受け取った keyboard 対応の mounted instance で、子 content が event を消費しなかった場合に閉じる。None instance への Escape 配達は約束しない。外部 handle、Escape、backdrop の hide は同じ lifecycle 経路を使う。Transition 未指定時と Exit なしの場合は即時に閉じ、Exit 指定時は完了までその instance の Surface・content・resource lease を保持する。Exit 完了後は破棄し、次の表示では factory を再実行する。

open は display ごとに `Closed | Pending | Mounted` lifecycle を持つ。repeated open は pending/mounted instance を activate、raise、recreate しない。exit 中の reopen は同じ mount generation・window・content を保持し、現在値と速度から enter へ retarget する。反転前の exit completion は無効化する。pending close は factory を呼ばず取消し、完全に閉じた後の genuine open は新 generation で開始する。stale factory result はその generation の window だけを回収し、callback 後に generation/runtime を再確認する。対象 instance close はその view/surface と lease のみ解放し、他 display や runtime の別 surface を閉じない。runtime shutdown/fatal は animation を即時取消し、pending を含む全 runtime-owned surface を cleanup し、fatal primary cause と attach host の所有権を維持する。display ごとの visibility/content teardown と runtime-wide resource sharing は独立に検証する。

## Geometry / Input API

### Bar

視覚的な content と native allocation は分離する。Bar の既定配置は下端だが、上端・左端・右端へ変更可能とする。以下の長手方向と厚み方向は、選択した辺に応じて入れ替わる。

- **Width policy（可逆的 default）**
  - 長手方向いっぱいに伸ばす配置と、content に合わせる floating 配置を扱う。
  - floating 配置では content の上限と Start／Center／End 相当の alignment を指定できる。native Surface の長手方向の allocation は伸ばしたまま、描画と入力を content に限定できる。

- **Margins / alignment**
  - logical margin で content の位置を決める。予約辺側の margin は Surface allocation と予約量にも含め、二重加算しない。
  - margin と thickness 個別の値だけでなく、それらの和も副作用前に検証する。

- **Reservation**
  - 自動予約は Bar に限定する。既定は厚み＋予約辺の margin、shadow は除外する。exclusive zone の明示的な上書きも許可する。
  - 予約は Enter 前から Exit 完了まで維持し、animation の進捗から再計算しない。非表示完了による Surface の破棄で解放する。一般の ShellSurface は予約を明示し、Overlay の既定値は0とする。

- **Display bounds / scale**
  - geometry は logical units で宣言する。display scale は platform 適用後のピクセル計算に反映する。複数 display では各 display の bounds/scale を独立に扱う。

- **Pointer hit area（実現性注意）**
  - input region は移動・clip 後の content bounds に限定し、透明な allocation 全体で入力を横取りしない。不可視入力層による回避策は用いない。

### Overlay

- **Placement**
  - default は現在の configured local viewport の中央。
  - explicit anchor + offset も最小 API で提供する。anchor は `Top | Bottom | Left | Right | Center` の組み合わせ、offset は logical units。
  - native request は display ごとの独立 layer-shell surface、四辺 anchor、requested size `0 × 0`、`exclusive_zone = 0` とする。配置・clamp・clip の基準は実際に configure された window-local viewport と display scale であり、global usable bounds/origin は利用可能と仮定しない。
- **Oversized content**
  - viewport を超える content はその configured local viewport 内で clip する。利用者は明示的に scroll container を選べる。自動的な文字サイズ縮小（typography scaling）や賢い reposition は行わない。
- **Scale**
  - 各 display の scale に応じて logical size を pixel size に変換し、clamp は pixel 換算後の configured viewport bounds で行う。
- **Reservation / Bar relation**
  - Overlay の既定 exclusive zone は0。予約付き Bar を避ける配置が必要な場合は、Overlay を開く前に Bar の初期 allocation/reservation が安定していることを要求する。
  - compositor の Layer order は物理的に Bar の背後へ積む保証ではない。同一 Layer の順序は compositor が決める。非予約 Bar は Overlay に隠れたり入力を妨げられたりし得る。予約の後続変化に対する adaptive relayout/resizing は保証しない。実際に viewport resize event を受けた場合は、その時点の viewport に追従する。

### Input modes

- **Keyboard mode**
  - `None`: surface creation mode `None` を使い、keyboard input を要求しない。keyboard 配達・Escape は約束しない。pointer click-through の指定でもない。
   - `OnDemand`: 通常の focus を要求する。これは BKSS の keyboard arbitration や OS activation/focus 成功保証ではない。実際の focus は compositor が決め、focus 取得・復帰を理由とする Surface 再作成はしない。

- **Outside-close backdrop（pointer interactive overlay）**
  - default OFF。opt-in のみ。
  - ON の場合、backdrop は content と同じ root/surface に描画し、入力領域は configured local viewport 内で content の外側に限定する。そこへの pointer down は close を発行して消費し、背後のアプリケーションには転送しない。display 外側や local viewport 外側を捕捉しない。
  - backdrop は別 surface/focus owner ではなく、独立した keyboard focus/arbitration を持たない。
  - default OFF の場合は backdrop paint/input を作らず、input region は content のみ。透明な full-display input layer は作らない。

- **Stacking**
  - backdrop と content は同じ root/surface で content 外側を backdrop paint/input とする。別 surface の物理 stacking、BottomBar より背後の配置、同一 layer order は保証しない。input region は上述の local viewport に限定する。

### 入力配達の受け入れ

pointer / keyboard の入力配達は CF-17 と CF-18 に従う。検証が必要な実現性・backend 固有の事項は、公開仕様ではなく実装者向け確認事項として記録する。

## 宣言と描画の境界

```text
State<T>                   // setup/mount で生成、clone は同じ cell を共有
Reactive<T>                // State の read-only view
Reactive<A>.map(f) -> Reactive<B>
Reactive<A>.combine(Reactive<B>, f) -> Reactive<C>
SourceState<T>              // 取得前 / 取得済みの値を表す状態
SourceDescriptor<T>         // 宣言時には接続せず、利用開始時に接続
ShellSurface / Bar / Overlay       // inert な設定・style・Transition
Surface の登録 + content factory   // display ごとに独立した mounted view を生成
Surface の handle                 // display 指定で表示・非表示
Shell::run(...) -> Result<(), ShellError>

Bar の厚み                       // 確定 logical px 専用型
GPUI Anchor / Layer / KeyboardInteractivity
GPUI Div / Styled / ParentElement  // content の構築・合成
Output の選択                     // 起動時の display snapshot、hotplug なし

GlassStyle { radius, opacity, tint, border, rim, inner_highlight, inner_shadow }
GlassStyle::with_hover(partial) -> GlassStyle
GlassStyle::with_pressed(partial) -> GlassStyle
GlassStyle::with_focus(partial) -> GlassStyle

Transition::fade()
Transition::slide_up() / slide_down() / slide_left() / slide_right()
Transition::duration(Duration) / spring(SpringPreset)
SpringPreset::Smooth / Snappy / Bouncy
transition / enter_transition / exit_transition
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

標準設定の適用対象は namespace `bkss-bottom-bar` の Bar とする。core の layer 生成は `LayerSurfaceConfig.options.namespace` を使用し、Overlay の `bkss-overlay` と分離する。`SurfaceDescriptor` の namespace は `bkss-core-surface` とする。

同一 surface 内の widget 間屈折は対象外とする。Overlay の backdrop と opacity animation は alpha による輪郭推定と干渉するため、標準設定例の対象に含めない。Overlay の入力・dismiss・animation は Surface の契約に従う。設定と適用上の制約は [Hyprglass 連携](hyprglass.md)を参照。

## animation

公開 Transition API は ui に置く。対象 property は opacity と logical translation X/Y。core runner が補間を実行し、runtime が lifecycle、スケジューリング、単一 completion を所有する。transition は Enter と逆方向 Exit の両方を設定し、後続 enter_transition／exit_transition はその側だけを置き換える。デフォルトでは animation なし。reverse_on_exit は提供しない。途中再表示は現在値から新しい target へ進み、endpoint へ jump しない。

Fade と上下左右の Slide を提供する。slide_up は下から、slide_down は上から、slide_left は右から、slide_right は左から現れる。Slide は固定 Surface 内の content の layout／hitbox を移動し、Surface 外への描画・移動は保証しない。既定距離は上下が測定した高さ、左右が幅。有限で非負の logical px に上書きできる。

最初の有効な layout で未変形 content の寸法を測定して plan を解決し、その時点から Enter の clock を開始する。初回 frame から初期 opacity／移動量を適用し、configure 待ちの間に animation を消費しない。距離は mounted instance ごとに固定し、次の mount で再解決する。placement／clip／input region は各 frame の viewport と測定結果へ追従する。

content の移動後 bounds と viewport の交差から input region を計算する。backdrop は固定し、opacity だけでは入力を無効化しない。複数 property の phase はすべての channel が完了した時に一度だけ完了する。opacity は描画値を [0,1] に clamp するが、translation の通常の overshoot は clamp しない。描画用 f32 で有限値として表現できない sample は error とする。

### timing kind

Easing と Spring は mutually exclusive にする。後続 timing 指定で切り替え、spring に duration を固定時間として併用しない。

```text
enum TimingConcept {
    Easing { duration: Duration, curve: EasingCurve },
    Spring { stiffness: f32, damping: f32, mass: f32, max_duration_ms: u64 },
}
```

### easing

- default duration: 160ms ease-out。
- duration は Duration の精度を保持し、ミリ秒未満を切り捨てない。0 は即時。
- easing 時は指定した duration を再適用する。

### spring（minimal physical model）

spring は damped harmonic oscillator の物理パラメータを公開する。これは GPUI native 実装の主張ではなく、BKSS ui レイヤーの仕様である。

UI は Smooth／Snappy／Bouncy の preset を提供し、core の spring solver の実行記述へ変換する。以下は実行記述の数値契約である。

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
| CF-02 | Glass | GPUI が tint・明示装飾・子 content を描画し、設定済み Hyprglass が Bar の背後の画面へ背景効果を適用する。要素ごとの背景 sampling・個別 intensity は保証しない |
| CF-03 | Bar | 起動時の複数 display に layer-shell Surface、型付き厚み、四辺の配置、予約、既定の非フォーカスを確認できる |
| CF-04 | Overlay | display 別 open/close、独立 visibility/content/lifecycle、definition 時の keyboard mode 選択、実際に配達された Escape による Interactive instance close を確認できる |
| CF-05 | lifecycle | exit 完了前に破棄せず、exit なしは即時 close、反復要求、現在値からの retarget、stale completion 防止をテストできる |
| CF-06 | animation | opt-in の opacity／translation X/Y を fake clock で検証し、未指定時は即時状態、複数 channel は単一 completion になる |
| CF-07 | reactive | test source の update、unsubscribe、resource sharing、cleanup を検証でき、最後の consumer 後に停止し次回利用で再取得する |
| CF-08 | displays | 起動時に接続済み複数 display を同時表示できる。hotplug は対象外と明記する |
| CF-09 | fatal integration failure | acquisition/subscription の開始時・開始後 failure が原因付きで伝播し、Shell 全体を終了する。cleanup は重複せず、shutdown 中に別 backend を開始しない |
| CF-10 | 初回取得と代替表示 | 遅延する test source でも Shell が表示され、取得前の代替表示から取得済みの表示へ更新される。空一覧との区別と再取得時の状態も検証できる |
| CF-11 | State/Reactive | State write、read-only Reactive、loading→ready、map/combine propagation を検証できる |
| CF-12 | display / single-seat keyboard | 二 display の独立 visibility/content teardown、共有 resource、None/Interactive creation mode と actual delivered-key recipient を検証できる。BKSS seat ordering/arbitration、native activation/focus restoration は保証しない |
| CF-13 | State/activation | ready 初期値、clone handle による同一 cell 更新、停止後 activation の stale update 破棄を検証できる |
| CF-14 | Shell 終了 | fatal cause/context を `Result<(), ShellError>` で返し、library が process exit せず、cleanup 中に decorative animation を待たないことを検証できる |
| CF-15 | Bar geometry | 二つの monitor size / 異なる logical scale で、予約の自動計算／明示上書き、full／floating content、logical margins、四辺 anchor を確認し、style h/w と Surface 厚みの分離、shadow の予約除外、Enter 前から Exit 完了までの予約保持を検証できる |
| CF-16 | Overlay geometry | configured local viewport 中央 default、explicit anchor/offset、configured local viewport/scale による pixel-clamp と clip、明示的 scroll container、自動 shrink/typography scaling なしを検証できる。global usable bounds は前提にしない |
| CF-17 | Input modes | 作成時の keyboard mode と actual key delivery を区別する。backdrop opt-in/default OFF、ON 時 local viewport 内 content-exterior click を消費し app に転送せず、OFF 時 input region が移動・clip 後の content のみであることを検証できる。native focus/activation、outside-viewport capture、Bar behind-order は保証しない |
| CF-18 | 入力配達の検証ゲート | 実装前に GPUI/Wayland の input-region/layer-surface capability と probe evidence を確認し、source/probe だけで native acceptance を主張しない。実装後は actual native example で BottomBar と Overlay の pointer/keyboard delivery を観察・記録する。headless/fake test は native 配達の証拠としない |
| CF-19 | Glass style contract | hover/pressed/focus の外観変化はユーザーが明示指定した場合のみ適用され、Glass 自体は自動効果を持たない。base -> hover -> pressed の順に partial override し、focus は最後に独立適用する。Glass style があるだけで container が focusable/clickable にならない。opacity と tint alpha の乗算ルールを検証できる |
| CF-20 | animation timing | Easing と Spring は mutually exclusive。easing default は160ms ease-out、Duration の精度を保持し、0は即時。途中反転は指定 duration を再適用する。SpringPreset が core solver の物理パラメータへ変換されることを検証できる |
| CF-21 | spring convergence | settle は正規化位置誤差・速度両方 eps = 0.001 未満、距離 0 かつ速度 0 のときのみ即時 completed、正規化 span は max(|original span|, 1.0)、max elapsed 2000ms per genuine request で endpoint snap + 単一 completion、redundant open は max をリセットせず stale completion は破棄しない、30/60/144Hz で同一 elapsed サンプリングの tolerance 内一致を検証できる |
| CF-22 | invalid params | 不正な spring・Glass・geometry・厚み・距離は panic や無言の sanitize ではなく副作用前に error で拒否する。厚み／margin の和も検証する。easing Duration の0は許可する。実行中に translation sample が描画型で有限値として表現不能になった場合も error とし、通常の overshoot は clamp しない |
| CF-23 | Glass reactive / immutable | `GlassStyle` は immutable clone input とし、reactive な個別プロパティ変更で再描画できる。Transition 未指定時は状態変化が即時反映することを検証できる |
| CF-24 | 宣言と生成 | Surface 宣言時は window／factory が動かず、display／mount ごとに fresh content を生成する。Styled は content に委譲し、native 設定は登録時に固定される |
| CF-25 | 測定と Slide | 最初の有効な layout で距離を解決し、その時点から Enter を開始する。初回 frame の opacity／移動量、移動後の子 hitbox と input region、viewport clip、固定 backdrop を headless test と native 観察に分けて確認する |

## 対象外

- display hotplug。初期要件は起動時に接続済みの display のみ。
- 他 compositor 向けの代替ガラス backend、同一 surface 内の widget 間屈折、Hyprglass の自動導入・ロード・設定検出。
- scale animation、および任意 subtree の入力領域、selection、caret/IME の変換。
- 特定 integration を core に組み込むこと、完成済み shell、未測定の性能保証。

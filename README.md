# BKSS

BKSS は Rust と GPUI を使い、Hyprland 向けのシェルを組み立てるためのライブラリです。`bkss-core` は Glass、runtime、reactive/resource、surface、animation の実行機構を、`bkss-ui` は BottomBar、Overlay、公開 Animation 定義を提供します。ガラスの背景効果には Hyprglass を使用します。特定の外部サービスとの統合や完成済みシェルを提供するものではありません。

## 主な機能と使い方

以下は現在の公開 API に基づく概要です。挙動と受け入れ条件は [foundation 仕様](docs/spec.md)、利用例は各 crate の `examples/` を参照してください。機能の実装と、各 display・scale・compositor 構成での実機検証は別です。

### 1. Shell と UI を組み立てる

Shell の runtime entry point は `bkss-core` に置き、アプリケーション全体の実行と終了を担います。利用側は宣言時に builder や外部 source の descriptor を組み立て、runtime の setup/mount で `State` や entity を作り、UI に結び付けます。実行前の descriptor は inert（外部接続を開始しない）で、source は UI consumer が利用を始めた時点で開始します。

`Shell::run` はアプリケーションを起動し、終了後に `Result<(), ShellError>` を返します。既存の GPUI アプリケーションには `Shell::attach` で接続できます。backend の接続・subscription failure は runtime 所有の resource・surface の cleanup と終了結果へ伝播し、library 自身はプロセスの終了コードを決めません。attach 先の host window と runtime 所有の window は区別されます。GPUI の component composition と BKSS の widget は併用できます。

### 2. State と外部データを UI に結び付ける

UI 内で作る値には、初期値がすぐ利用できる `State<T>` を使います。clone した handle は同じ値を共有し、どれからの更新も同じ State に反映されます。読み取り側には read-only view の `Reactive<T>` を渡します。

外部データは「取得前」と「取得済み」を `SourceState<T>`（`Loading` / `Ready(T)`）で区別します。画面は source の初回取得を待たずに表示でき、取得前の代替表示と取得後の内容を binding します。取得済みの空一覧や `Ready(None)` は有効な値で、`Loading` とは別です。

`Reactive` の基本操作は次の通りです。

- `map`: wrapper を含む値全体を純粋・同期的に変換します。
- `combine`: 2つの最新値を合成します。
- `map_ready`: `SourceState` の `Loading` を保持し、`Ready` の値だけを変換します。
- `combine_ready`: 両入力が `Ready` になるまで `Loading` を保持します。
- `observe`: 値の publication を callback で受け取ります。返された `ReactiveSubscription` を保持し、不要になったら破棄して購読解除します。observer の owner が破棄された場合も callback は停止します。

明示的な更新は値が等しくても dependent に伝わります。複数 State の更新は atomic transaction ではありません。filter や async map は初期 scope に含まれません。

外部 resource は runtime の範囲で共有されます。同じ resource を複数の active UI が使え、source handle の clone だけでは consumer になりません。最後の consumer が終了すると subscription/resource は停止し、次の activation では再取得・再接続して初期状態から始まります。終了済み activation から遅れて届いた更新は新しい状態に反映されません。

`RuntimeHandle::mount` が返す `MountedBinding` を content が保持することで consumer の寿命を表します。read-only な Reactive view を保持するだけでは consumer を維持しません。

別 thread からは `SourcePublisher::thread_bridge` が返す `ThreadPublisher` で `Send` な値・error payload を送り、UI thread 上で source の値・error へ変換します。source の値・error 自体に `Send` は必須ではありません。返された `ThreadPublisherGuard` は source の guard に保持し、その破棄で配送を停止します。送信成功は enqueue の成功であり、UI への配送完了保証ではありません。

### 3. Glass を内容に適用する

`Glass` は視覚装飾のプリミティブで、style を適用しただけでは内容を focusable/clickable にしません。`GlassStyle` で radius、opacity、tint、任意の border/rim、inner highlight/shadow を指定します。opacity は Glass 装飾に適用し、子 content の不透明度には適用しません。tint の alpha と opacity は乗算されます。

hover / pressed / focus の見た目は、状態別 style を明示した場合だけ変化します。状態別 style は base への部分上書きで、適用順は base → hover → pressed、focus は最後に独立して適用します。Glass 自体は状態変化をアニメーションさせません。

背景の取得・ぼかし・屈折・光沢は **Hyprglass** が、薄い tint と子 content は GPUI が描画します。標準 tint の alpha は 0.15 です。明示的な border/rim/inner effect は通常の GPUI 装飾として利用できますが、Hyprglass の光沢と重ねる場合は二重の縁に注意してください。

Hyprglass の導入・ロードと compositor 設定は別途必要です。[設定手順と対象範囲](docs/hyprglass.md)を参照してください。標準の設定例は `bkss-bottom-bar` だけを対象とし、Overlay は含みません。同一 surface 内の widget 間屈折と、他 compositor 向けの代替 backend は提供しません。library は Hyprglass の自動ロード・状態検出・設定変更を行いません。

### 4. BottomBar と Overlay を配置する

`bkss-ui` の `RuntimeHandleExt::mount_bottom_bar` と `OverlayRuntimeHandleExt::mount_overlay` で、既存の runtime に widget を登録します。宣言の作成だけでは window や外部接続は開始しません。

**BottomBar** は display 下端の常設 layer-shell surface です。`BottomBarGeometry` で論理高さ、margin、予約の有無を指定し、幅は `BottomBarWidth::FullDisplayWidth` または内容幅に合わせる `FloatingContentWidth` を選びます。内容幅の場合は最大幅と `Center` / `Left` / `Right` の alignment を指定できます。geometry は logical units で定義され、予約高さは bar の高さと bottom margin の合計で、shadow は含みません。予約は既定で有効です。複数 display は起動時に同時に扱います。

**Overlay** は `OverlayDefinition` を登録し、返された `OverlayHandle` の `open(display, cx)` / `close(display, cx)` で開閉します。同じ定義でも content、表示状態、lifecycle は display ごとに独立します。配置は実際に configure された window-local viewport の中央が既定で、edge と logical offset による anchor も指定できます。viewport を超える内容は clip されるため、スクロールが必要なら scroll container を明示します。

対象は起動時に接続済みの display です。display hotplug は対象外です。

Keyboard mode は definition 作成時に `KeyboardFocus::None` または `Interactive` を明示します。surface 作成時の mode はそれぞれ `None` / `OnDemand` です。`None` はポインタのクリックを背後へ通す意味ではありません。Overlay の外側クリックによる close は `PointerDismiss::Backdrop` を明示した場合だけ有効になり、local viewport 内の content 外側でクリックを消費します。既定の `Disabled` では backdrop を作らず、input region は content のみです。

keyboard の初期契約は single Wayland seat です。BKSS は overlay 間の arbitration、activation、focus 復帰、同一 layer 内の表示順を制御せず、実際の focus と順序は compositor が決めます。実際に Escape を受け取った `Interactive` instance は、子 content が event を消費しなければ閉じます。backdrop の別 surface や、BottomBar より背後への物理配置は保証しません。

### 5. 必要な箇所だけ animation を指定する

`Animation` は `bkss-ui` の opt-in な opacity 定義で、`OverlayDefinition::with_animation` で指定します。`enter` / `exit` / `transition` を定義し、実行・スケジューリング・lifecycle は core runtime が所有します。未指定の場合と exit 定義がない場合の close は即時です。`transition` は enter と逆向きの exit を表します。scale animation は対象外です。

timing は easing または spring のいずれかです。easing は duration と curve を指定し、既定は 160ms ease-out、duration 0 は即時です。spring は stiffness、damping、mass、最大時間で指定します。exit 完了までは同じ surface・content・resource lease を保持し、途中の reopen は現在値と速度から retarget します。fatal shutdown は animation 完了を待たずに取消・cleanup します。

## Examples

- [BottomBar](crates/bkss-ui/examples/bottom_bar.rs)
- [静的 Overlay](crates/bkss-ui/examples/static_overlay.rs)
- [opacity animation](crates/bkss-ui/examples/animated_overlay.rs)
- [遅延 source と fallback 表示](crates/bkss-ui/examples/source_fallback_display.rs)

各 example の存在は、すべての compositor・display 構成での実機受け入れ完了を意味しません。

# BKSS

BKSS は Rust と GPUI を使い、Hyprland 向けのシェルを組み立てるためのライブラリです。`bkss-core` は Glass、runtime、reactive/resource、Surface、animation の実行機構を担います。`bkss-ui` は GPUI を拡張する宣言的 SDK で、公開範囲を **Shell・Effects・Animation** に絞ります。layout や汎用 widget は GPUI をそのまま利用し、ガラスの背景効果には Hyprglass を使用します。

完成したデスクトップアプリケーションは [`crates/shell`](crates/shell/README.md) にあります。ビルドは workspace root から `mise x -- cargo build --release -p shell --bin shell`、実行ファイルは `target/release/shell` です。必要なシステムライブラリ・フォント・起動設定はアプリケーションの README を参照してください。

## 主な機能と使い方

以下は現在の公開 API に基づく概要です。挙動と受け入れ条件は [foundation 仕様](docs/spec.md)、利用例は各 crate の `examples/` を参照してください。機能の実装と、各 display・scale・compositor 構成での実機検証は別です。

### 1. Shell と UI を組み立てる

Shell の runtime entry point は `bkss-core` に置き、アプリケーション全体の実行と終了を担います。利用側は宣言時に builder や外部 source の descriptor を組み立て、runtime の setup/mount で `State` や entity を作り、UI に結び付けます。実行前の descriptor は inert（外部接続を開始しない）で、source は UI consumer が利用を始めた時点で開始します。

`Shell::run` はアプリケーションを起動し、終了後に `Result<(), ShellError>` を返します。GPUI アプリケーションには `Shell::attach` で接続できます。backend の接続・subscription failure は runtime 所有の resource・Surface の cleanup と終了結果へ伝播し、library 自身はプロセスの終了コードを決めません。attach 先の host window と runtime 所有の window は区別されます。

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

`bkss_ui::Glass::new()` は新しい GPUI element で、`Styled` と `ParentElement` により角丸・padding・子要素等を合成できます。style を適用しただけでは内容を focusable/clickable にしません。

独自の装飾や reactive な style が必要な場合は、view が `bkss_ui::effects::MountedGlass` を保持し、render ごとに `element(state, cx)` で新しい element を生成します。`GlassStyle` で radius、opacity、tint、任意の border/rim、inner highlight/shadow を指定できます。opacity は Glass 装飾に適用し、子 content の不透明度には適用しません。tint の alpha と opacity は乗算されます。

GPUI の container style と Glass 装飾の設定は別です。装飾自体の半径は `GlassStyle.radius` で指定し、container の角丸だけで装飾半径が置き換わるわけではありません。

hover / pressed / focus の見た目は、状態別 style を明示した場合だけ変化します。状態別 style は base への部分上書きで、適用順は base → hover → pressed、focus は最後に独立して適用します。Glass 自体は状態変化をアニメーションさせません。

背景の取得・ぼかし・屈折・光沢は **Hyprglass** が、薄い tint と子 content は GPUI が描画します。標準 tint の alpha は 0.15 です。明示的な border/rim/inner effect は通常の GPUI 装飾として利用できますが、Hyprglass の光沢と重ねる場合は二重の縁に注意してください。

Hyprglass の導入・ロードと compositor 設定は別途必要です。[設定手順と対象範囲](docs/hyprglass.md)を参照してください。標準の設定例は `bkss-bottom-bar` だけを対象とし、Overlay は含みません。同一 surface 内の widget 間屈折と、他 compositor 向けの代替 backend は提供しません。library は Hyprglass の自動ロード・状態検出・設定変更を行いません。

### 4. ShellSurface・Bar・Overlay を配置する

Surface 宣言の `mount(&runtime, factory, cx)` で登録します。宣言の作成だけでは window や外部接続は開始しません。factory は display／mount ごとに view を生成し、その render で GPUI の `div()` 等を通常どおり組み立てます。Surface 宣言自体を child として描画しても別 window が生成される設計ではありません。

**Bar** は上下左右の画面端に配置し、登録時に対象 display への表示を開始します。`Bar::new(Thickness::new(px(48.))?)` の厚みは確定 logical px 専用型です。auto／割合指定を受け付けず、不正な数値も Surface 生成前に拒否します。`.edge(Anchor::TOP)`、margin、full length／floating、alignment 等を指定できます。

Bar の予約量は厚み＋予約辺の margin で、shadow は含みません。`.exclusive_zone(...)` で上書きでき、予約は Enter 前から Exit 完了まで維持します。Hyprglass の対象指定に使う namespace は `bkss-bottom-bar` です。

**Overlay** は既定では登録時に非表示です。返された `SurfaceHandle` の `show(display, cx)`／`hide(display, cx)` で開閉します。既定は Layer::Overlay、予約0、透明背景、keyboard None、Transition なし。Glass は自動挿入しません。同じ宣言でも content、表示状態、lifecycle は display ごとに独立します。

**ShellSurface** は native anchor・size・Layer 等を明示する基礎宣言です。Surface 設定は登録時に固定されます。どの宣言でも GPUI の style API は内部 content に適用され、`.h()`／`.w()` は native Surface の厚み・予約を変更しません。

content の配置は configured local viewport を基準とし、中央・anchor と logical offset を選べます。viewport を超える内容は clip されるため、スクロールが必要なら content に scroll container を明示します。

対象は起動時に接続済みの display です。display hotplug は対象外です。

keyboard は `.keyboard_interactivity(KeyboardInteractivity::OnDemand)` 等で指定します。`None` はポインターのクリック透過とは別設定です。既定の input region は content のみで、外側を背後へ通します。Overlay の `.dismiss_on_backdrop()` を明示した場合だけ、local viewport 内の content 外側クリックを消費して閉じます。

keyboard の初期契約は single Wayland seat です。BKSS は Surface 間の arbitration、activation、focus 復帰、同一 Layer 内の表示順を制御せず、実際の focus と順序は compositor が決めます。keyboard 対応 instance が実際に Escape を受け取り、子 content が消費しなかった場合は閉じます。別 backdrop Surface や Bar より背後への物理配置は保証しません。

### 5. 必要な箇所だけ animation を指定する

`.transition(Transition::fade())` や `Transition::slide_up()` 等で Fade／上下左右の Slide を指定します。`transition` は Enter と逆方向 Exit の両方を設定し、後続の `enter_transition`／`exit_transition` はその側だけを上書きします。`.and(...)` で合成できます。Scale と `reverse_on_exit` は提供しません。

Slide は固定 Surface 内の content を動かします。既定距離は測定した高さ／幅、`.distance(...)` で logical px を指定できます。最初の有効な layout で距離を確定し、その時点から Enter を開始するため、configure 待ちで animation 時間を消費しません。距離は mount ごとに固定し、content の配置・入力領域は viewport に追従します。

timing は easing または spring です。`.duration(Duration)` はミリ秒未満の精度も保持し、既定は160ms ease-out、0は即時です。途中反転は現在値から指定 duration を再適用します。`.spring(SpringPreset::Smooth)` 等では core の spring solver が現在値と速度から retarget します。

Exit 完了までは同じ Surface・content・resource lease を保持し、途中の再表示では再利用します。完了後は破棄し、次回の show で factory を再実行します。未指定の場合と Exit がない場合の hide は即時です。fatal shutdown は animation 完了を待たずに取消・cleanup します。

## Examples

- [Bar](crates/bkss-ui/examples/bottom_bar.rs)
- [静的 Overlay](crates/bkss-ui/examples/static_overlay.rs)
- [Transition](crates/bkss-ui/examples/animated_overlay.rs)
- [遅延 source と fallback 表示](crates/bkss-ui/examples/source_fallback_display.rs)

各 example の存在は、すべての compositor・display 構成での実機受け入れ完了を意味しません。

## ライセンス

BKSS 自身のコードは [MIT License](LICENSE) で提供します。依存ライブラリとシステムフォントには、それぞれのライセンスが適用されます。[第三者依存と配布上の注意](docs/third-party-licenses.md)を参照してください。

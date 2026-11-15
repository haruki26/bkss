# Integration の利用

`bkss-integration` は外部の状態と操作、`bkss-ui` はシェルに組み込む表示要素を提供します。状態取得には `bkss-core` の `Source` と resource lifecycle を利用します。

## プリセット

| 宣言 | 接続先・対象 |
| --- | --- |
| `SourceDescriptor::new(Clock::new(ClockPrecision::Minute))` | ローカル時刻。`Second` も選択可能 |
| `UsageSource::new(UsageKind::Cpu)` | CPU 使用率 |
| `UsageSource::new(UsageKind::Memory)` | メモリ使用量 |
| `UsageSource::new(UsageKind::Storage)` | ストレージ使用量 |
| `Hyprland::new(socket_dir)` | Hyprland のワークスペース・アクティブウィンドウ |
| `BatterySource::new()` | UPower のシステム用バッテリー |
| `Wifi::new()` | NetworkManager |
| `Bluetooth::new()` | BlueZ |
| `BacklightSource::new()` | `/sys/class/backlight` |
| `Audio::new()` | PulseAudio または PipeWire-Pulse の出力・入力 |
| `Tray::new()` | StatusNotifier のホストと DBusMenu |
| `Notifications::new()` | freedesktop 通知サーバー |
| `GpuSource::new()` | DRM sysfs の GPU 情報。計測は AMD を対象とする |

宣言だけでは接続しません。同じ runtime 内で同じ型と key を持つ source は、最初の mount で開始され、その後の mount と共有されます。

## 表示へ接続する

fallible な content factory の中で binding を取得し、表示 entity に渡します。

```rust
let descriptor = Wifi::new();
// content factory 内
let binding = runtime.mount(&descriptor, cx)?;
let view = cx.new(|cx| SourceDisplay::new(binding, cx));
```

`SourceDisplay` は通常の GPUI Entity として配置できます。表示 entity が binding を保持し、最後の consumer が解放されると取得処理の停止を要求します。descriptor、状態の clone、操作 handle の保持だけでは取得を延命しません。

`SourceDisplay::new` は状態の `IntegrationView` を利用します。

```rust
pub trait IntegrationView {
    fn render(
        &self,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> gpui::AnyElement;
}
```

`with_render` で標準描画を、`with_loading` で初回取得前の表示を変更できます。`IntegrationView` を実装していない値には `SourceDisplay::new_with_render` を利用します。空一覧や未接続など、取得済みの正常な状態は Loading と区別されます。

アイコンと値は GPUI の要素として組み合わせます。SVG を利用するアプリケーションは GPUI にアセットを登録してください。BKSS はアイコン一式やアセット登録を提供しません。

## 標準描画と操作 UI の分担

各 integration の取得済み状態には、簡単な標準表示を返す `IntegrationView` が実装されています。音声は現在の音量等、tray と通知は件数などを表示します。`SourceDisplay::new` でその標準描画を利用できます。

音量ボタン、tray メニュー、通知一覧などの integration 固有の操作 UI は提供しません。利用側は `with_render` または独自の GPUI Entity で表示を構成し、型付き状態の操作 API を呼びます。操作待ち・エラー表示・メニュー開閉などの表示状態も利用側が所有します。

通知の受信を継続する間は binding を保持してください。個別通知の終了とサーバーの停止は別です。サーバーは `org.freedesktop.Notifications` の既存所有者を置き換えず、競合を source failure として返します。

tray は既存 watcher があればホストとして参加し、なければ置き換えなしで watcher を開始します。アイコンの画像データ・名前、状態、メニュー情報と操作 API を公開します。アイコン描画・テーマ名の解決・メニューの表示は利用側で実装してください。アイコン名を無条件にファイルパスとして扱わないでください。SNI 操作に渡す座標は利用側で指定します。

コンパイル対象の [`source_display.rs`](../crates/bkss-ui/examples/source_display.rs) は、時計と自作 Source、アイコン＋値、描画差し替えを示します。

## 非同期操作

操作は取得済みの状態から利用します。例えば Wi-Fi の操作は `state.operations`、音声の操作は `state.control`、Bluetooth とバックライトの操作は状態自身が提供します。

```rust
let operation = state.operations.enable()?;
operation.await?;
```

最初の Result はキューへの受付結果、await 後の Result は操作結果です。`DispatchError` は停止済みまたはキュー満杯を、`OperationError` は activation の終了またはバックエンドの失敗を区別します。待機 future を破棄しても、実行開始済みの操作の取消は保証しません。

Hyprland の既存操作は `Hyprland::switch_workspace(socket_dir, "2")?.await?` です。

## 認証

Wi-Fi は `WifiState::next_auth_request().await` から要求を受け取り、要求の `responder` に `WifiCredentials` を渡します。入力値には move-only の `Secret<String>` を使用します。Bluetooth は状態の型付き Agent1 要求に応じ、PIN、passkey、確認・許可への応答を渡します。

Wi-Fi の入力を中止する場合は `WifiAuthRequest::cancel()` を呼びます。未完了の要求を取り消し、同じ要求の responder clone も無効にします。回答済みの認証や開始済みの接続操作を取り消す機能ではありません。

responder の clone は一つの応答権を共有します。回答済み、キャンセル済み、期限切れ、終了済み要求への回答は拒否されます。応答の受付は認証成功そのものを意味しません。入力画面とエラー表示は利用側が所有します。

BlueZ のエージェントは BKSS から開始する操作用であり、自動的にシステムの既定エージェントにはしません。NetworkManager のエージェントは秘密情報を永続保存しません。秘密情報をログや通常の公開状態へ入れないでください。

## 実行環境と制約

- 各 source に対応するシステム／セッションサービスが必要です。認証や操作には polkit 等のシステム側の許可が必要になる場合があります。
- 音声のビルド・リンクには `libpulse`、デバイス監視には `libudev` の開発用環境が必要です。
- バックライト変更は sysfs の書き込み権限に従います。BKSS は権限昇格や外部の特権コマンド実行を行いません。
- GPU の未対応ベンダーや欠測項目は、対応状態や `Option` で区別します。未取得を架空の 0% として扱いません。
- 使用量と GPU は内部サンプリングを用います。時計は指定精度の時刻境界で更新します。
- source の接続・購読失敗は既存の Shell failure 契約に従います。通常の操作失敗とハードウェアの不在は区別します。

## 自作 integration とモジュール

自作の取得処理は公開 `Source` を実装します。標準表示が必要なら、その `Value` に `IntegrationView` を実装してください。別 thread からの更新には既存の `SourcePublisher::thread_bridge` を利用できます。

ライブラリ内部は `src/base/` に共通契約、`src/presets/<integration>/` に各連携を配置しています。`mod.rs` は module 宣言と再エクスポート専用です。単体テストは対象実装ファイル内、公開 API の統合テストは crate 直下の `tests/` に配置します。

使用量の取得処理は `src/presets/usage/{cpu,memory,storage,gpu}/` にまとめています。公開する `UsageSource` と `GpuSource` は共通の Source／worker 基盤を利用しますが、GPU 固有の複数デバイス・欠測情報は専用の状態型で維持します。

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

`Notifications::new()` と `with_address()` は、送信側の `expire_timeout = -1` に既定の 10 秒を適用します。`Notifications::with_default_timeout(endpoint, timeout)` ではこの既定期限を指定できます。`NotificationsEndpoint::Session` と `Duration::ZERO` を指定すると、期限をサーバーに委ねた通知は自動終了しません。送信側が明示した正の期限は引き続き適用し、明示的な 0 は無期限です。endpoint と既定期限が一致する source だけが activation を共有します。ポップアップを隠す時刻は、この通知本体の期限とは別に利用側で管理してください。

tray は既存 watcher があればホストとして参加し、なければ置き換えなしで watcher を開始します。メインアイコンの名前と画像データは `Properties.GetAll` の snapshot から取得し、片方が欠けていてももう片方を保持します。両方空でも項目を保持します。アイコンの画像データ・名前、状態、メニュー情報と操作 API を公開します。アイコン描画・テーマ名の解決・メニューの表示は利用側で実装してください。アイコン名を無条件にファイルパスとして扱わないでください。SNI 操作に渡す座標は利用側で指定します。

コンパイル対象の [`source_display.rs`](../crates/bkss-ui/examples/source_display.rs) は、時計と自作 Source、アイコン＋値、描画差し替えを示します。

## 非同期操作

操作は取得済みの状態から利用します。例えば Wi-Fi の操作は `state.operations`、音声の操作は `state.control`、Bluetooth とバックライトの操作は状態自身が提供します。

```rust
let operation = state.operations.enable()?;
operation.await?;
```

最初の Result はキューへの受付結果、await 後の Result は操作結果です。`DispatchError` は停止済みまたはキュー満杯を、`OperationError` は activation の終了またはバックエンドの失敗を区別します。待機 future を破棄しても、実行開始済みの操作の取消は保証しません。

Hyprland の workspace 切り替えには `Hyprland::switch_workspace(socket_dir, "2")?.await?` を使います。名前付き通常 workspace は Hyprland の dispatcher selector `name:<name>`（例: `"name:chat"`）で指定します。取得状態は `workspaces` と `special_workspaces` に分かれ、通常 workspace の `id` は名前付きの場合 `None` です。`active_workspace` は `ActiveWorkspace::Normal` または `ActiveWorkspace::Special` で種別を保持します。`clients` は全 workspace の `HyprlandClient` を含み、`workspace_address` で所属先を示します（所属先がない場合は `None`）。

切り替え要求は Hyprland 0.57.2 の Lua dispatcher 形式 `/dispatch hl.dsp.focus({ workspace = ... })` を使用します。数値は Lua number、名前はエスケープした Lua string として渡し、compositor が返す失敗を操作結果へ伝えます。旧形式の `dispatch workspace ...` は送信しません。

`active_workspace` はフォーカス中モニターで表示中の special workspace を優先します。Hyprland の `activeworkspace` が背後の通常 workspace を返す場合も、`monitors` の `specialWorkspace` から空でない address を取得します。ウィンドウのない special も同じ扱いです。表示中 special がなければ通常の `activeworkspace` へ戻ります。出力別の状態一覧ではなく、グローバルなフォーカス状態です。

## 外部プロセスからの command IPC

利用側が所有する有限のコマンド名を Unix socket で受け取る source と client を提供します。登録した名前だけを受け付け、任意のプロセス実行や UI 操作は行いません。

```rust
let descriptor = CommandIpc::new(
    runtime_dir,
    "powermenu.sock",
    ["open", "shutdown"],
)?;
// mount 後、Ready の CommandIpcState を取得
let request = state.next_request().await;
if let Some(request) = request {
    let command = request.command();
    let payload = request.payload();
    // 利用側が処理してから明示的に返答
    request.respond(b"handled".to_vec())?;
}
```

`CommandIpcClient::send(socket_path, command, payload)` は同期 API なので UI thread ではなく worker から呼びます。`runtime_dir` は絶対パスの、現在のユーザー所有かつ mode `0700` の既存ディレクトリである必要があります。socket は mode `0600` で作成されます。

source は `<socket_path>.lock` の排他を稼働中保持します。lockfile は空の自己所有通常ファイルで mode `0600` とし、終了後も削除しません。稼働中に手動削除すると排他が別の inode に分かれるため、削除しないでください。

既存 endpoint は、自己所有・mode `0600` の Unix socket で、時間制限付き接続確認が接続拒否となり、検査時と同じ device・inode の場合だけ回収します。稼働中の listener、通常ファイル、symlink、不正な所有・権限、差し替えられた endpoint は削除しません。確認のタイムアウトや取消も、既存 endpoint を保持したまま失敗します。

runtime directory の検証と socket の bind は source worker が行い、成功後に初期 `Ready` を publish します。起動時の失敗は source failure として runtime に伝わります。

コマンド名は ASCII の英数字・`-`・`_`・`.` からなる最大 128 bytes で、request/response payload の上限は各 64 KiB、応答待ちは 2 秒です。未知名・不正 request・満杯は拒否されます。IPC の成功応答は、要求が利用側へ届けられ応答を受けたことだけを示し、UI 処理の実行や成功を保証しません。

## 認証

Wi-Fi は `WifiState::next_auth_request().await` から要求を受け取り、要求の `responder` に `WifiCredentials` を渡します。入力値には move-only の `Secret<String>` を使用します。Bluetooth は状態の型付き Agent1 要求に応じ、PIN、passkey、確認・許可への応答を渡します。

Wi-Fi の入力を中止する場合は `WifiAuthRequest::cancel()` を呼びます。未完了の要求を取り消し、同じ要求の responder clone も無効にします。回答済みの認証や開始済みの接続操作を取り消す機能ではありません。

responder の clone は一つの応答権を共有します。回答済み、キャンセル済み、期限切れ、終了済み要求への回答は拒否されます。応答の受付は認証成功そのものを意味しません。入力画面とエラー表示は利用側が所有します。

BlueZ のエージェントは BKSS から開始する操作用であり、自動的にシステムの既定エージェントにはしません。NetworkManager のエージェントは秘密情報を永続保存しません。秘密情報をログや通常の公開状態へ入れないでください。

## 実行環境と制約

- 各 source に対応するシステム／セッションサービスが必要です。認証や操作には polkit 等のシステム側の許可が必要になる場合があります。
- 音声のビルド・リンクには `libpulse`、デバイス監視には `libudev` の開発用環境が必要です。
- バックライト変更は sysfs の書き込み権限に従います。BKSS は権限昇格や外部の特権コマンド実行を行いません。
- 使用量は内部サンプリングを用います。時計は指定精度の時刻境界で更新します。
- source の接続・購読失敗は Shell 全体の fatal error とし、runtime が resource と Surface を終了します。通常の操作失敗とハードウェアの不在は区別します。

## 自作 integration とモジュール

自作の取得処理は公開 `Source` を実装します。標準表示が必要なら、その `Value` に `IntegrationView` を実装してください。別 thread からの更新には `SourcePublisher::thread_bridge` を利用できます。

`bkss_integration::base::{spawn_worker, Cancellation, WorkerGuard}` は独自 Source でも使える共通の worker 基盤です。`Source::start` で作った bridge と publisher を `spawn_worker` に渡し、返された guard を Source の lifetime に保持します。guard の Drop は UI 配送を閉じて停止を要求するだけで、worker の join や I/O 完了を待ちません。worker の非同期待機は `Cancellation::cancelled()` と race させ、`is_cancelled()` で停止を確認します。独自の取得処理が作った子プロセスや接続は、その worker 側で回収してください。

ライブラリ内部は `src/base/` に共通契約、`src/presets/<integration>/` に各連携を配置しています。`mod.rs` は module 宣言と再エクスポート専用です。単体テストは対象実装ファイル内、公開 API の統合テストは crate 直下の `tests/` に配置します。

使用量の取得処理は `src/presets/usage/{cpu,memory,storage}/` にまとめています。GPU 固有の取得・デバイス型・ツール連携は preset として提供せず、利用側が独自 Source として実装します。

# bkss-integration

## 構成と責務

- `src/lib.rs` は公開 API の再エクスポートを担う。内部 module の移動だけで crate root の import を変えない。
- `src/base/` は worker・キャンセル、bounded な操作キュー、認証 responder、`IntegrationView` の共通契約を所有する。新しいプリセットで同じ実行機構を作り直さない。
- `src/presets/<integration>/` が取得・操作・状態・標準描画を所有する。CPU／Memory／Storage／GPU は `presets/usage/` にまとめ、GPU の複数デバイス・未対応・欠測は専用型で保持する。
- 各 `view.rs` は取得済み状態の標準 `IntegrationView` を実装する。render 中に接続・購読・非同期操作を開始しない。操作付きの完成 UI やメニュー開閉状態は利用側が所有する。

## 実行と外部プロトコル

- `Source::start` の UI thread では bridge の作成と worker 起動までに留める。接続・登録・初期取得は worker 内で行い、イベント購読は初回 snapshot より先に開始する。
- `WorkerGuard` の Drop は UI 配送を閉じて停止を要求するだけで、join や I/O 完了を待たない。非同期の接続・呼び出し・待機は `Cancellation` と race させる。`thread_bridge` は無制限キューなので高頻度イベントは集約する。
- `base::command` のキューは worker 内でキャンセルトークンと共に作り、取得済み状態に操作 handle を渡す。handle は activation を延命しない。受付時の `Busy`／`Inactive` と、受付後の操作失敗を区別する。
- source の key に接続先や開始挙動を変える設定を含める。同じ型・key の descriptor は取得処理を共有する。
- イベントを持つ backend を外部コマンドの定期実行で代替しない。時計は時刻境界、使用量は内部サンプリングを使う。接続・購読の故障と、機器不在・未接続・操作拒否を混同しない。
- 通知サーバーの名前は置き換え・待ち行列なしで取得する。tray は既存 watcher があれば host として参加する。BlueZ のアプリ用 agent を自動的に既定 agent にしない。
- 認証 responder は clone 間で一度だけ回答でき、取消・期限切れ・終了後は無効になる。秘密文字列には `Secret<T>` を使い、秘密情報を公開状態・ログ・永続保存へ入れない。agent は呼び出し元を検証し、回答待ち中に Cancel／Release を妨げる lock を保持しない。
- D-Bus の fixture も公式の引数・戻り値・property 契約に合わせる。ローカル実装と同じ誤った wire signature を fixture に複製しない。

## 検証

workspace root から実行する。

```sh
mise x -- cargo test -p bkss-integration
mise x -- cargo test -p bkss-integration --test wifi_source
mise x -- cargo test -p bkss-integration --lib presets::usage
mise x -- cargo clippy -p bkss-integration --all-targets --all-features -- -D warnings
```

- D-Bus の protocol test は private bus を起動するため、実サービスへの接続は不要。`dbus-daemon` は Wi-Fi・Bluetooth・tray の試験で必須で、通知の試験だけは未導入時に skip する。
- ビルドには `libpulse`／`libudev` の開発用ライブラリと `pkg-config` が必要。backlight の実操作には sysfs の書き込み権限が必要で、テストのためにユーザーの実機設定を変更しない。
- GPU の公開テストは descriptor 構築の確認であり、実機の計測を検証しない。private bus・合成 sysfs・headless GPUI の成功を実機対応済みの根拠にしない。
- root の Lefthook は core／ui の設定だけを読むため、この crate の検証を hook 任せにしない。

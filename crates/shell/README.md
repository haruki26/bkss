# Glass デスクトップシェル

BKSS と GPUI を使用する Hyprland 向けデスクトップアプリケーション。BKSS workspace の `shell` package としてビルドする。dotfiles や AGS のソースは不要。

## 機能

- Glass の上下バー、ワークスペース、システム状態、トレイ。
- 通知バナーと、アプリごとにまとめた通知一覧。
- 月表示、日付選択、月移動、今日へのリセット。予定表サービスとの連携は行わない。
- 天気、ロック、電源メニュー。

## 実行環境

Hyprland の Wayland セッション、互換性のある Hyprglass、セッション D-Bus、ビルド用の Rust・`pkg-config`・libpulse・libudev が必要。各システム連携は対応するサービス・権限も必要とする。天気の通信には `curl`、ロックには `hyprlock`、電源操作には `systemctl` を使用する。

シェルのアイコンを表示するには `Symbols Nerd Font` が必要。本文には HackGen を指定している。フォントは同梱しない。アイコン用フォントがない場合に SVG で代替表示することはない。

通知の既定アクションは利用できるが、表示名だけからアプリを推測して起動する機能は提供しない。

実行時には `XDG_RUNTIME_DIR`、`WAYLAND_DISPLAY`、`HYPRLAND_INSTANCE_SIGNATURE` が設定されていること。

## ビルドと起動

以下のコマンドは BKSS の workspace root で実行する。

```sh
mise x -- cargo build --release -p shell --bin bkss
./target/release/bkss run
```

ユーザー環境にインストールする場合は、workspace root で `mise run install-local` を実行する。既定のインストール先は `~/.cargo/bin/bkss`。タスクは `PATH` を変更しないので、必要なら Cargo の `bin` ディレクトリを自分で `PATH` に追加する。インストール版を使う場合は、自動起動と `bkss quit` に同じインストール済みバイナリを使う。

通知サービスが競合するため、既存の AGS シェルなど別の通知デーモンとの同時起動は避ける。AGS を使っている場合は、事前に `ags quit`（独自インスタンスは `ags quit --instance <名前>`）で終了する。起動ラッパーだけを停止しても通知を所有するプロセスが残る場合がある。

Hyprland の自動起動には、この checkout の `target/release/bkss` の絶対パスと引数 `run` を指定する。引数なしの起動はできない。電源メニューのキーバインドには同じ実行ファイルと引数 `toggle powermenu` を指定する。初回起動前と Rust ソースの変更後には再ビルドする。設定の再読み込みだけでは、自動起動の再実行やアプリケーションの再起動は行われない。

起動と同じバイナリで `./target/release/bkss quit` を実行すると、シェルに終了を要求する。このコマンドは終了要求の受理を確認し、プロセスの終了完了までは待たない。

再起動するとメモリ上の未読通知は失われる。

電源メニュー用の `$XDG_RUNTIME_DIR/bkss.sock` と、`quit` 用の実行ファイルごとのソケットは別です。強制終了後などに残ったソケットは、安全に回収できる場合に次の起動で回収します。排他用のロックファイルは終了後も残る正常なファイルです。手動で削除しないでください。

実行中のシェルの電源メニューを切り替える。

```sh
./target/release/bkss toggle powermenu
```

## Glass の設定

Glass の背景効果には、互換性のある Hyprglass の導入と Hyprland 設定が必要。[導入手順](../../docs/hyprglass.md)に従ってプラグインをロードした後、Hyprland の Lua 設定に次を追加する。library の example 用設定とは対象 namespace が異なる。

```lua
local hg = assert(hl.plugin.hyprglass, "Hyprglass plugin failed to load")
hg.config({
    enabled = false,
    layers = { enabled = false },
    subsurfaces = { enabled = false },
})
for _, namespace in ipairs({
    "bkss-top",
    "bkss-bottom",
    "bkss-banners",
    "bkss-calendar",
    "bkss-power",
    "bkss-tray",
}) do
    hg.layer(namespace, {
        preset = "pomme",
        mask_mode = "contour",
        mask_threshold = 0.1,
        live_resample = true,
    })
end
hg.layer("bkss-notifications", { exclude = true })
hg.config({ layers = { enabled = true } })
```

通常の window と subsurface は対象にしない。通知センターの surface は画面全体を覆うため除外する。既存の layer 設定を更新する場合は、上記の namespace に合わせる。自動インストール・reload は行わない。

BKSS の example にある `bkss-bottom-bar` は example 専用の名前であり、このシェルの名前とは異なる。通常の半透明描画だけでは、背景のぼかし・屈折が成立したことにはならない。反映後は `hyprctl configerrors`、`hyprctl plugin list`、`hyprctl -j hyprglass status` と実画面を確認する。

## 通知の保持

ポップアップは 10 秒で非表示になるが、送信側が期限を指定しない通常の通知は、通知センターに未読として残る。通知の削除や操作は引き続き可能。送信側が明示的な期限を指定した通知は、その期限で終了する。

## GPU 使用率

下部 usage に GPU 使用率を表示する。取得処理は `src/sources/gpu/` に置く shell 独自の Source で、BKSS の公開 base worker・キャンセル・UI 配送基盤を利用する。複数 GPU はデバイス名順に表示し、未対応・欠測は `—` とする。

AMD は sysfs の busy 値を使用する。Intel i915 は `intel_gpu_top` の JSON 出力を使用し、最も busy なエンジンクラスの値を表示する。クラスを合算した GPU 全体の総稼働率ではない。Intel の取得にはツールのインストールと perf 監視権限が必要で、シェルから権限設定を自動変更しない。ツールや権限が利用できなければ欠測表示になる。

`intel_gpu_top` に `CAP_PERFMON` を付与する方法は、その実行ファイルの利用者に perf 監視権限を与えるため、管理者が影響を確認して設定する。パッケージ更新で capability が失われる場合がある。取得不能になったデバイスへの再試行はシェルの再起動、またはデバイスの再接続で行う。

## 天気

`$XDG_CONFIG_HOME/bkss/settings.json`（未指定時は `~/.config/bkss/settings.json`）から天気設定を読み取る。設定例:

```json
{
  "weather": {
    "openweathermap_api_key": null
  }
}
```

API キーはこの項目か環境変数 `OPENWEATHERMAP_API_KEY` に設定する。ファイル内の値が優先される。設定ファイルがなくても起動でき、環境変数の値を使用する。両方なければ天気は未設定状態になる。読み取り不能・不正 JSON・空白だけのキーは設定エラーとして扱い、ファイルの内容をエラーメッセージに含めない。

設定はリポジトリの外に置き、ファイルは所有者だけが読み書きできる権限にする。誤配置対策として workspace root と `crates/shell` 直下の `settings.json` は Git の除外対象としている。AGS の設定は読み込まない。認証情報をソースや追跡対象ファイルへ記入しない。

位置情報は GeoClue を使用し、取得できない場合は `ipwho.is` に問い合わせる。天気は OpenWeatherMap から取得する。利用には外部サービスへの通信が発生する。

## 確認

```sh
mise x -- cargo fmt --all -- --check
mise x -- cargo clippy --workspace --all-targets --all-features -- -D warnings
mise x -- cargo test --workspace
```

実画面ではバーの配置、Glass の背景効果、通知の到着・操作、カレンダー、電源メニューの表示・終了、キーボード入力を別途確認する。ビルドや単体テストの成功だけでは、ネイティブ表示・入力の確認を代替できない。

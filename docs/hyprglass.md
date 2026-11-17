# Hyprglass 連携

## 責務と前提

BKSS は Hyprland を対象とする。背景の取得、ぼかし、屈折、色収差、光沢は [Hyprglass](https://github.com/hyprnux/hyprglass) が描画し、GPUI は形状、薄い tint、子 content、入力を担当する。他 compositor 向けの代替ガラス backend は用意しない。

設定の基準は **Hyprglass v1.1.1 / Hyprland 0.57 系**。プラグインは Hyprland の内部描画 API に依存するため、Hyprland と関連ライブラリに対応する版を使用する。更新時には互換性と描画を再確認する。版チェックの強制回避はしない。

library は Hyprglass のインストール、ロード、検出、設定ファイルの変更、reload を実行しない。導入済みでも無効・停止状態では背景効果は出ない。通常の GPUI 描画だけを液体ガラスとして扱わない。

## 導入と設定

1. [公式の導入案内](https://github.com/hyprnux/hyprglass/tree/v1.1.1#installation)に従って、実行中の Hyprland に対応するプラグインを導入する。
2. Hyprland の Lua 設定でプラグインをロードする。
3. ロード行の後で [設定例](../examples/hyprglass.lua)を読み込む。

次のパスは配置例であり、実際の導入先と checkout の絶対パスに置き換える。

```lua
hl.plugin.load(os.getenv("HOME") .. "/.local/lib/hyprglass/v1.1.1/hyprglass.so")
dofile("/absolute/path/to/bkss/examples/hyprglass.lua")
```

設定例は `bkss-bottom-bar` のみを対象に `pomme` preset、`contour`、`mask_threshold = 0.1`、背景更新への追従を指定する。通常 window と subsurface への効果は無効にする。ほかに `hg.layer()` の登録がある場合はそれらも対象となるため、既存の設定と合わせて対象を管理する。

プラグインが未ロードなら設定例はエラーにする。ただし設定の評価成功は shader の生成成功・実際の描画成功の証拠ではない。

### 反映と状態確認

Lua 設定を保存し、Hyprland の設定を reload する。

```sh
hyprctl reload
hyprctl configerrors
hyprctl plugin list
hyprctl -j hyprglass status
```

v1.1.1 の `hg.layer()` は reload の完了時に対象 namespace と個別設定を確定する。`hyprctl eval` が `ok` を返しただけでは適用済みと判断しない。

`status` の `versionCheck`、`features.layers.active` と `reason` を確認する。shader は最初の描画まで `pending` の場合がある。Bar を表示した後に `shaders = "ready"` と実描画数を確認する。

```sh
mise x -- cargo run -p bkss-ui --example bottom_bar -- --seconds 30
hyprctl layers
hyprctl hyprglass stats
```

`hyprctl layers` の namespace は `bkss-bottom-bar` となる。runtime 所有のほかの surface は `bkss-overlay`、低レベルの `SurfaceDescriptor` は `bkss-core-surface` であり、この設定では対象にしない。

無効化する場合は設定例の最後を `hg.config({ layers = { enabled = false } })` に変更して reload する。ロード行だけ残して設定を削除すると、Hyprglass の既定値で通常 window への効果が有効になる可能性がある。完全に止める場合は plugin のロード設定も外す。

## Glass の描画

- 標準 tint は黒の alpha 0.15、opacity は 1.0、radius は 12 logical px。標準で GPUI の border / rim / inner effect を付けない。
- `tint.alpha × opacity` は GPUI 装飾の alpha であり、Hyprglass の効果量ではない。子 content の opacity は変えない。
- `contour` は surface の画素の alpha から輪郭を推定する。合成後の alpha が threshold 以下の背景はガラス領域にならず、影や不透明な子要素も輪郭に影響する。
- 明示的な border / rim / inner effect は引き続き GPUI 装飾として描画する。Hyprglass の光沢と重ねる場合は縁が二重にならないよう調整する。
- 同一 surface 内の widget 間屈折は保証しない。`glass_surface` などの複数カード例は GPUI 装飾の確認用であり、個別カードの背景屈折を保証する例ではない。

## 今回適用しないもの

Overlay は対象に含めない。画面全体を暗くする backdrop の alpha 0.4 は contour の対象になり、画面全体に効果が広がる。opacity animation では背景が threshold を跨ぎ、輪郭が途中で変化しうる。領域指定と遷移を確認せずに `bkss-overlay` を追加しない。入力領域・外側クリック・animation は Surface の契約に従う。

この設定は常用時の可読性、GPU 時間、消費電力、複数 display / mixed scaling の性能保証ではない。背景の屈折と文字の読みやすさは実際の背景で調整する。

## 参照

- [Hyprglass v1.1.1 README](https://github.com/hyprnux/hyprglass/blob/v1.1.1/README.md)
- [v1.1.1 の設定処理](https://github.com/hyprnux/hyprglass/blob/v1.1.1/src/PluginConfig.cpp)

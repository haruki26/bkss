# bkss-ui

## 構成と責務

- `src/lib.rs` は公開 feature module `shell`、`effects`、`animation` と crate root の便利な再公開を提供する。利用者は意図的に `bkss_ui::shell`、`bkss_ui::effects`、`bkss_ui::animation` の各 namespace を使える。
- `shell` は ShellSurface／Bar／Overlay の宣言と登録、`effects` は Glass の公開 UI API、`animation` は Transition／SpringPreset の定義・検証と core の実行記述への変換を担う。
- feature 内では宣言・検証・geometry 計算と GPUI 描画・入力の副作用を責務に応じて分ける。内部実装を配置や test の都合で公開しない。
- `tests/` は公開 API の統合 test、`examples/` は利用例・実環境の動作確認を担う。private な実装の単体 test は対象の実装ファイル内に置く。

## Shell

- ShellSurface／Bar／Overlay は明示的に runtime へ登録する inert な Surface 宣言とする。通常の GPUI 子 Element として描画するだけで独立した native Surface を生成しない。
- Surface 設定と root content の style を分離する。`Styled` は内部の GPUI content に委譲し、再生成・display 別生成が必要な content は factory／view で構築する。
- Anchor／Layer 等は GPUI の型を利用・再公開し、同じ機能を再実装しない。
- Bar の Surface の厚みは確定 logical px 専用の型で指定し、auto／割合指定を受け付けない。GPUI の `h`／`w` は内部 content に適用し、Surface の厚みや予約量を上書きしない。厚みの数値検証は副作用前に行う。
- exclusive zone の自動計算は Bar に限定し、厚みと予約する辺の margin を含め、shadow は含めない。手動上書きも許可する。予約は Enter 前から Exit 完了まで維持する。
- Overlay の既定値は Layer::Overlay、exclusive zone 0、透明背景、keyboard interactivity None、Transition なしとする。一般の ShellSurface の予約設定は明示する。
- Overlay の既定の pointer input region は content のみとし、外側は背後へ通す。外側クリックで閉じる backdrop を明示した場合は、その領域内の外側クリックを消費し、背後へ転送しない。
- keyboard None と pointer のクリック透過を混同しない。OnDemand は表示直後の自動 focus や focus 復元を保証しない。同じ Layer 内の native stacking order も保証しない。

## Effects

- Glass の tint／装飾 opacity は子 content の opacity や Hyprglass の効果量とは別である。
- Overlay の backdrop や opacity animation と Hyprglass の効果領域の整合は別途検証が必要であり、Bar 向け設定をそのまま Overlay へ適用しない。

## Animation と lifecycle

- `transition()` は Enter と逆方向の Exit の両方を設定する。後続の `enter_transition()`／`exit_transition()` は対応する側だけを上書きする。`reverse_on_exit()` は提供しない。
- v0.1 の描画対象は Fade／Slide とする。Slide は固定 Surface 内の content を動かし、Surface 境界で clip する。既定距離は上下方向が content の高さ、左右方向が幅とし、logical px で上書き可能にする。Surface 自体の画面外移動は保証しない。
- 任意 subtree の Scale は入力・clip・IME 等との整合を含む後続の技術検証対象とし、v0.1 の API から除外する。
- SpringPreset は Smooth／Snappy／Bouncy を提供し、core の spring solver へ変換する。Spring の途中反転では現在値・速度を引き継ぎ、厳密な時間逆再生を要求しない。
- duration 付き easing の途中反転は現在値から開始し、指定 duration を再適用する。Spring と固定 duration は別の timing として扱う。
- runtime が安定した controller／instance 世代と状態を所有し、短命な Element 参照を Surface の識別に使わない。Exit 完了前は content を保持し、途中の再表示では再利用する。
- Exit 完了後は Surface と mounted content を破棄し、次回表示で factory を再実行する。古い completion が新しい instance を破棄してはならない。
- animation 中の入力領域も描画との整合を保つ。

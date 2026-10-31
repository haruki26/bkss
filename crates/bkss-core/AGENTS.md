# bkss-core

## 構成と責務

- `src/lib.rs`: 非公開の feature module を宣言し、公開 API を再公開する。
- `src/animation/`: animation の実行記述、時刻を受け取る runner、補間・spring 計算。
- `src/reactive/`: State／Reactive、通知・購読、派生値。
- `src/resource/`: Source、activation の共有、scope／lease、thread 間の更新配送、停止と failure。
- `src/glass/`: style の検証・解決、reactive な描画更新、GPUI 描画。
- `src/surface/`: 接続前の宣言、display 選択・readiness、低レベル設定とエラー。
- `src/shell/`: runtime と binding、resource／window の lifetime 管理、animation のスケジューリング。
- `tests/`: 公開 API の統合テスト。
- `examples/`: 利用例・実環境の動作確認。

`src/surface/` は接続前の宣言・設定、`src/shell/surface/` は surface の生成・終了などの実行機構を担う。

## 依存と所有

- 内部依存は `shell → animation/surface/resource/reactive`、`resource → reactive`、`glass → reactive` を基本とする。`surface` と `reactive` は他の feature module に依存させない。
- GPUI は恒久的な依存とし、置き換えだけを目的とした抽象化は作らない。
- 純粋な判断と副作用は、明瞭さやテストに役立つ場合に feature 内で分離する。
- surface 宣言は runtime 登録前に接続・window 作成を開始しない。
- `ResourceScope` が activation を所有し、lease／publisher は scope を延命しない。mounted consumer は `MountedBinding` の保持で表す。
- runtime 所有の window と attach 先の host window を区別する。

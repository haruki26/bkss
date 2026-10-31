# bkss-ui

## 構成と責務

- `src/lib.rs`: 非公開の module を宣言し、公開 API を再公開する。
- `src/animation/`: 公開 Animation／Timing 定義、検証、core の実行記述への変換。
- `src/layout/`: BottomBar／Overlay など、shell 上の配置と表示を担う component。
- `src/layout/<component>/mod.rs`: component の宣言・公開 API と runtime への登録。
- `src/layout/<component>/geometry.rs`: geometry の値・検証と配置計算。
- `src/layout/<component>/view.rs`: GPUI 描画、入力領域、pointer／keyboard の処理。
- `src/layout/overlay/`: Overlay の宣言と描画。core runtime の animation sample を content wrapper の opacity に適用する。
- `src/shared/`: 複数 component で同じ契約を持つ共通実装。現時点では空。
- `src/ui/`: 独立した UI component。現時点では空。
- `tests/`: 公開 API の統合テスト。
- `examples/`: 利用例・実環境の動作確認。

## 依存と所有

- `bkss-ui → bkss-core` を依存方向とし、core から UI への依存を作らない。
- `shared` は component に依存させない。共通化は契約が一致する実装に限り、似た処理だけを理由に異なる挙動を統合しない。
- GPUI と core の Glass・reactive・surface API を利用し、window／resource の lifecycle 管理を UI 側で重複実装しない。
- animation の実行・スケジューリング・completion による surface の終了は core runtime が所有し、UI は定義と sample の描画反映を担う。
- component の宣言は runtime 登録前に接続・window 作成を開始しない。content factory の実行時期は core の lifecycle に従う。
- mounted content が必要な `MountedBinding` を保持する。factory には descriptor と weak runtime handle を渡し、不要な lease や strong runtime handle を捕捉しない。
- geometry の検証と描画・入力の副作用は責務に応じて分け、内部実装を配置やテストの都合で公開しない。
- private な geometry・描画・入力の単体テストは対象の実装ファイル内に置く。公開 API だけで成立する lifecycle の検証は `tests/` に置く。

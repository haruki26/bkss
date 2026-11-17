# AGENTS.md

BKSS は Rust と GPUI で Linux Wayland 向けシェルを組み立てるライブラリです。以下の規則は workspace 全体に適用します。作業対象に最も近い `AGENTS.md` も確認してください。

## 設計方針

- BKSS は GPUI を拡張する宣言的な desktop shell SDK です。GPUI が提供する layout・style・描画・子要素合成を再実装しません。
- component は composition で拡張し、`Styled`／`ParentElement` で GPUI の API と合成します。汎用の Container／Row／Column／Modal／Tooltip や UI preset は提供しません。
- GPUI は恒久的な依存とし、置き換えだけを目的とした抽象化は作りません。GPUI の型は意図して公開 API の一部とする場合に公開します。
- 同じ機能を持つ別経路や互換層を追加しません。共通化は契約が一致する実装に限り、似た処理だけを理由に異なる挙動を統合しません。
- 新しい capability を追加する前に、どの層が責務を所有するか確認してください。純粋な判断・検証と副作用は、明瞭さやテストに役立つ境界で分離します。
- 公開 API の変更時は、利用箇所・example・test・関連仕様を整合させてください。

## Workspace の構成と責務

- Rust 2024 edition の Cargo workspace とし、`crates/bkss-core`、`crates/bkss-integration`、`crates/bkss-ui` で構成します。
- `bkss-core` は Glass の style・reactive 更新・GPUI 描画、runtime、reactive/resource、低レベルの Surface と lifecycle、animation の実行機構を担います。
- `bkss-integration` は core の Source 契約を実装し、外部情報の取得・操作と、取得済み状態の標準 GPUI 描画契約を担います。UI crate には依存しません。
- `bkss-ui` の公開範囲は Shell、Effects、Animation です。Surface 宣言と登録、Glass の公開 UI API、Transition の定義と sample の描画反映を担います。
- `bkss-ui → bkss-core`、`bkss-integration → bkss-core` の依存方向を守り、core から上位層への依存や循環依存を作りません。
- 外部 IPC や特定 integration を core に持ち込まず、外部 backend の取得・操作や compositor 通信を UI に実装しません。backend の詳細はそれを所有する層の境界内に置きます。
- crate 固有のモジュール構成や実装上の注意は、その crate 内の `AGENTS.md` に記載してください。workspace 全体の設計方針は本書に集約します。

## Runtime と所有

- Surface 宣言と source descriptor は inert な値とし、runtime 登録前に接続や window 作成を開始しません。content factory の実行時期は core の lifecycle に従います。
- core runtime が window／resource の lifetime、frame scheduling、animation completion による Surface の終了を所有します。UI 側に同じ実行機構を作りません。
- core runner が補間・spring 計算を行い、UI は実行記述の定義と計算済み sample の描画反映を担います。
- `ResourceScope` が activation を所有し、lease／publisher は scope を延命しません。mounted consumer は `MountedBinding` の保持で表します。
- mounted content が必要な binding を保持します。factory には descriptor と weak runtime handle を渡し、不要な lease や strong runtime handle を捕捉しません。
- runtime 所有の window と、`Shell::attach` 先の host window の所有権を区別します。

## 描画と Platform

- 対象 compositor は Hyprland です。v0.1 の Output は起動時に接続済みの複数 display を対象とし、hotplug は対象外です。
- GPUI が Glass の形状・tint・装飾・content を描画し、Hyprglass が compositor 側の背景効果を担当します。背景取得・blur・refraction を通常の GPUI 装飾で代替したと主張しません。
- 同一 Surface 内の要素ごとの背景 sampling・屈折や、個別 intensity の compositor への反映は v0.1 では保証しません。
- native 入力配達・focus・効果領域の成立は、source 調査や headless test だけで受け入れ済みとせず、実環境で確認します。

## 仕様

- [foundation / BKSS-UI 仕様](docs/spec.md) を公開挙動と受け入れ条件の基準としてください。
- 内部の計画・調査記録と、実装・試験による受け入れの証拠を区別してください。

## Rust Code Style

Rust の変更前に [Rust Code Style](docs/rules/rust-code-style.md) を必ず確認してください。

## Code Quality

- 既存の構成と抽象化を優先し、変更は目的に必要な範囲に留めてください。
- format・lint の警告をエラーとして扱います。通すだけの抑制を追加せず、必要な抑制は事前に相談してください。
- 既存の抑制は依頼なく一律に変更しないでください。

## Verification

コマンドは workspace root で `mise x --` を使って実行してください。

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

- 変更に応じた狭い確認を先に行い、workspace 全体に影響する Rust 変更では上記を実行してください。crate 固有の確認方法があればそれも使います。
- ドキュメントのみの変更ではリンク、パス、コマンド、差分を読み取り専用で確認し、Cargo の実行は通常不要です。
- 実行できなかった確認は理由とともに報告してください。
- `mise install` は定義済みツールと hook の導入用で、通常の検証には不要です。`.cargo/config.toml` の `lint` alias はファイルを書き換えるため使わないでください。
- Lefthook は format / clippy の修正を stage します。hook を CI の仕様とみなさず、読み取り専用の上記コマンドの代わりにしないでください。

## Boundaries

### Always do

- 作業対象に最も近い `AGENTS.md` を確認し、その適用範囲の規則に従ってください。
- 既存のユーザー変更を保持し、変更の範囲を依頼に合わせてください。
- 挙動を変えた場合は適切な確認を実行し、失敗や未実施を正確に報告してください。

### Ask first

- 新規依存や大きな依存更新、workspace 設定・toolchain・mise・`.cargo`・Lefthook の変更。
- 破壊的な公開 API 変更、新 crate の追加、責務の移動、unsafe の導入、アーキテクチャ変更。
- lint 抑制など、既存の品質基準を変える変更。

### Never do

- 生成物や vendored file を手編集したり、失敗を隠す目的で検証を弱めたりしないでください。
- `unwrap` / `expect` / `panic!` を通常の recoverable error handling に使わないでください。
- 内部実装を意図せず公開したり、core から UI への依存や crate 間の重複実装を作ったりしないでください。
- 無関係な refactor や lockfile の変更を混ぜないでください。承認済み依存変更に必要な lockfile 更新は除きます。

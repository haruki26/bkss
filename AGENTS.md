# AGENTS.md

BKSS は Rust と GPUI で Linux Wayland 向けシェルを組み立てるライブラリです。以下の規則は workspace 全体に適用します。作業対象に最も近い `AGENTS.md` も確認してください。

## Workspace Principles

作業前に [Workspace Principles](docs/rules/workspace.md) を必ず確認してください。

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

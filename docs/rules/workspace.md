# Workspace Principles

## 構成と責務

- BKSS は Rust 2024 edition の Cargo workspace で、`crates/bkss-core` と `crates/bkss-ui` をメンバーとします。
- `bkss-core` は Glass、runtime、reactive/resource、低レベルの surface と lifecycle 機構を担います。
- `bkss-ui` は core の上に構築し、公開 UI API と再利用可能な汎用 widget を担います。両 crate は循環依存させません。
- 外部 IPC や特定 integration を core に持ち込まず、framework や backend の詳細はそれを所有する抽象化の背後に置きます。GPUI の型を公開するのは、意図して公開 API の一部とする場合に限ります。
- 新しい capability を追加する前に、どの層がその責務を所有するか確認してください。

## foundation 仕様

- [foundation 仕様](../spec.md) を公開挙動と受け入れ条件の基準としてください。
- `docs/.agent/core-foundation/` の計画・確認記録は実装者向け情報です。そこで報告した API 調査や試験結果を、foundation の実装済みまたは受け入れ済みとみなさないでください。
- crate 固有のモジュール構成や規約が必要になった場合は、その crate 内の `AGENTS.md` に記してください。

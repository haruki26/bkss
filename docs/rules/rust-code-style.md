# Rust Code Style

## Rules

### Error Handling

- 通常の処理では `Result` と `?` でエラーを伝播し、境界で十分な文脈を加えてください。呼び出し側が区別する必要のある失敗は、適切な型付きエラーで表現します。
- 型付きエラーは、呼び出し側が失敗の種類に応じて処理を分ける公開境界で使ってください。内部の各関数に専用エラー型を増やす必要はありません。既存の型付きエラーを、利便性だけのために文字列へ変換して情報を失わないでください。
- エラーには失敗した操作と必要な対象の文脈を加え、元の原因を可能な限り保持してください。秘密情報や入力全体など、診断に不要な機密データをエラーやログに含めないでください。
- `unwrap`、`expect`、`panic!` を通常のエラー処理に使わないでください。テストや probe では、明確な不変条件に基づく場合に限り使用できます。
- 特定のエラーライブラリを前提にせず、既存の依存と crate の慣習に合わせてください。

#### Shell の fatal error と cleanup

- Shell を継続できない backend の接続・subscription failure は、panic ではなく原因を保持した `Result` として Shell の呼び出し側へ返してください。library は `process::exit` を呼ばず、終了コードや最終出力は呼び出し側が決めます。通常の failure を `catch_unwind` で隠さないでください。
- fatal shutdown 中の cleanup failure は、元の fatal cause を置き換えず、二次的な failure として扱ってください。独立して実行できる cleanup は best effort で続けますが、外部 backend の故障中に全 cleanup が成功することは保証しません。
- 「取得中」「空の一覧」「未接続」など仕様上有効な domain state、および通常の lifecycle cancellation を、接続・subscription failure と混同しないでください。

### Ownership and Responsibility

- 振る舞いは、それを所有する型やモジュールに置いてください。handler は処理を調整し、複雑な状態遷移や計算は適切な所有者に分けてください。
- 繰り返されるロジックは共通化し、似た実装を複数箇所に増やさないでください。

### Module Organization

- すべての `mod.rs` はモジュール宣言とリエクスポート専用としてください。型・関数・定数・`impl`・テストなどの定義は置かず、責務に応じた実装ファイルに置いてください。

### Public APIs

- BKSS の概念を表す小さく明確な API を優先し、型と構成で関係を明示してください。
- 内部実装を偶然公開せず、公開 API は既存の crate の境界と一貫させてください。
- 破壊的な API 変更は事前に相談してください。

### Dependencies

- 標準ライブラリと既存の依存で解決できるかを先に確認してください。些細な用途の依存や重複機能を追加しないでください。
- 依存方向を明確に保ち、下位層が上位層に依存しないようにしてください。

### Logging and Debugging

- 製品コードに `println!`、`eprintln!`、`dbg!` を残さないでください。意図した probe や example の出力は除きます。
- 既存の logging 基盤がない場合、導入前に相談してください。ログに秘密情報を含めず、一時的な診断出力は作業後に削除してください。
- library は global logger/subscriber やログ出力先を初期化しません。伝播する同じエラーを各層で重複して記録せず、最終的な対応方針を決める境界で必要な診断を行ってください。これは有用な debug span や必要な計測を禁じるものではありません。
- Shell library 自身は fatal error を印字したり終了コードを決めたりせず、呼び出し側が `Result` に基づいて判断します。

### Tests

- 単体テストは、実装と同じファイル内の `#[cfg(test)] mod tests` に記述してください。
- 統合テストは、workspace 内の各 crate のルートディレクトリにある `tests/` 配下に作成してください（例: `crates/bkss-core/tests/`）。

### Error-path Verification

- エラー経路のテストでは、原因と必要な操作・対象の文脈が呼び出し側まで保たれること、契約上副作用前に拒否する入力が副作用を起こさないことを確認してください。
- fatal shutdown では primary error の保持、独立した cleanup の実行、二重 cleanup や二重 shutdown がないことを確認してください。通常の domain state と lifecycle cancellation が fatal path に入らないことも検証してください。

### Performance

- hot path に不要な高コスト処理や blocking 処理を置かないでください。不要な allocation、clone、再構築を避けます。
- 性能最適化は測定に基づいて行い、未検証の性能保証をしないでください。

### Unsafe Rust

- safe Rust で表現できない明確な必要性がある場合に限り、unsafe を使用してください。
- unsafe block は必要最小限とし、安全性を保つ不変条件を近くに記してください。

### Comments and Documentation

- コメントはコードの言い換えではなく、非自明な制約や理由を説明してください。
- 公開 API には、必要に応じて lifecycle や ownership など呼び出し側が守る契約を記してください。

# 第三者依存と配布上の注意

## ライセンスの範囲

BKSS 自身のコードは root の [MIT License](../LICENSE) で提供する。依存ライブラリ・システムライブラリ・フォントを MIT に再許諾するものではない。各依存物には、それぞれのライセンスが適用される。

本リポジトリは Cargo が外部から取得する依存ソース、システムライブラリ、フォントファイルを同梱しない。ソースリポジトリの公開と、依存コードの vendoring・実行ファイル・コンテナの配布は区別する。後者では、実際に含まれる成果物に対応するライセンス本文・著作権表示・NOTICE・ソース提供等の条件を確認する。

## GPUI と Zed

GPUI は Zed の revision `c83abe7d0e060de08db386fbf86f7e95bfe6cb09` に固定している。

- [`gpui`](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/Cargo.toml)、[`gpui_platform`](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui_platform/Cargo.toml)、[`gpui_linux`](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui_linux/Cargo.toml) の package 宣言は Apache-2.0。
- ライセンス本文: [固定 revision の LICENSE-APACHE](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/LICENSE-APACHE)。
- Zed リポジトリ全体の GPL 表示だけで、各 GPUI package のライセンスを判断しない。依存 revision・feature を変えた場合は、取り込まれる package とそのライセンスを再確認する。

Apache-2.0 の依存物を再配布する際には、該当するライセンス本文・表示・NOTICE 等の条件を別途満たす必要がある。

## ネイティブライブラリ

`bkss-integration` が使用する Rust binding と、そのリンク先のライセンスは別である。

| 対象 | 確認したライセンス |
| --- | --- |
| `libpulse-binding` / `libpulse-sys` | MIT OR Apache-2.0 |
| `udev` / `libudev-sys` | MIT |
| PulseAudio のライブラリコード | LGPL-2.1-or-later の表示あり |
| systemd の libudev コード | LGPL-2.1-or-later の表示あり |

参照: [PulseAudio の source header](https://www.freedesktop.org/software/pulseaudio/doxygen/sample_8h_source.html)、[systemd libudev](https://github.com/systemd/systemd/blob/main/src/libudev/libudev.c)。

バイナリ配布時は、実際のリンク方法・同梱ライブラリ・バージョンに基づいて LGPL の条件を確認する。Rust binding が MIT であることだけでは、リンク先の配布条件は決まらない。

## フォントとアイコン

`shell` は利用者の環境にある `Symbols Nerd Font` と HackGen を指定して描画する。フォントファイルと旧 AGS の SVG アイコンは本リポジトリに同梱しない。

フォント名や glyph のコードポイントを参照することと、フォントファイルを複製・配布することは異なる。フォントを配布物へ追加する場合は、[Nerd Fonts のライセンス](https://github.com/ryanoasis/nerd-fonts/blob/master/LICENSE)と[各 glyph set のライセンス一覧](https://github.com/ryanoasis/nerd-fonts/blob/master/license-audit.md)、元フォントの条件を確認する。Nerd Fonts 全体を単一ライセンスとして扱わない。

## 依存調査の限界

2026-11-19 時点の Cargo.lock（SHA-256: `3f22fc55a09b2b64efc815b2a8ed04f2b5c254826c400199700f75686ffb86aa`）には 708 package があり、このうち workspace 自身の 4 package は MIT。第三者の 704 package は取得済みの package manifest と照合でき、ライセンス宣言の未確認はなかった。`cargo metadata --locked --offline --all-features` でも workspace の構成と依存解決を確認した。

照合した宣言に GPL のみを要求する package は見つからなかった。`self_cell` の `Apache-2.0 OR GPL-2.0-only` は Apache-2.0 を選択できる。MPL-2.0 の `cbindgen`・`dwrote`・`option-ext`、Unicode-3.0 の ICU 系、NCSA を含む複合ライセンス等は、それぞれの条件を維持する。これらを含む依存物を一律に MIT として扱わない。

Cargo.lock は全プラットフォーム向け・開発用の依存も含み、Linux の実行ファイルに含まれるコードの一覧とは一致しない。package manifest の SPDX 表記の照合だけで、すべてのファイル・資産の権利関係やバイナリ配布条件まで保証するものではない。

依存の更新・feature の変更・バイナリ配布を行う場合は、最終的な依存グラフと成果物を対象に再確認する。

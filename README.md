# Dioryga

システム構築・運用プロジェクトのためのインフラ台帳。サーバー・ネットワーク機器の購入計画から、構築・本稼働・保守・改修・廃棄までを、プロジェクト単位でQCD（品質・コスト・納期）とともに一元管理する。

**「今どうなっているか」ではなく「どう変わってきたか」を第一級のデータとして扱う**点に、他の構成管理ツールとの違いがある。

## 状態

実装着手前。仕様は `docs/spec/` を参照。

## リポジトリ構成

| | |
|---|---|
| 本リポジトリ | 実装 |
| [volanja/Dioryga_Design](https://github.com/volanja/Dioryga_Design) | 要件定義・基本設計・設計上の議論記録 |

## ドキュメント

- [`CLAUDE.md`](CLAUDE.md) — 技術スタック、破ってはいけない不変条件、v1スコープ
- [`docs/spec/schema.md`](docs/spec/schema.md) — 全テーブル・全カラム
- [`docs/spec/vocabularies.md`](docs/spec/vocabularies.md) — enum値の語彙
- [`docs/spec/validation.md`](docs/spec/validation.md) — アプリケーション層の検証規則

未解決項目の一覧と設計判断の経緯は、設計リポジトリ側で管理する（[`docs/design/open-questions.md`](https://github.com/volanja/Dioryga_Design/blob/main/docs/design/open-questions.md)）。

## ライセンス

Copyright (c) 2026 volanja

**[MIT](LICENSE-MIT) と [Apache-2.0](LICENSE-APACHE) のデュアルライセンス。**利用者はいずれかを選択できる。Rustエコシステムの慣行に沿った形であり、Apache-2.0が明示的な特許許諾を与える一方、Apache-2.0と非互換なGPLv2のプロジェクトへはMITを選んで取り込める。

明示的に別段の記載がない限り、本リポジトリへの貢献は上記デュアルライセンスの下でライセンスされるものとする（Apache-2.0 §5）。

依存するオープンソースソフトウェアのライセンス表示はバイナリに埋め込んである。全文は次で出力できる。

```bash
dioryga licenses
```

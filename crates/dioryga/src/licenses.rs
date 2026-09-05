//! ライセンス表示。
//!
//! **単一バイナリで配布するため、ライセンスの全文を見る経路がバイナリ以外に
//! 存在しない。**MIT・Apache-2.0・BSD等はいずれも著作権表示とライセンス全文の
//! 保持を求めており、ソースを配らない以上、バイナリ自身が持つほかない。
//!
//! # `rust-embed` ではなく `include_str!` を使う
//!
//! 静的アセット（`assets/`）は`rust-embed`で埋め込んでいるが、ここでは使わない。
//! **`rust-embed`は`debug_assertions`が有効なときファイルシステムから読む**ため、
//! 開発ビルドでは「埋め込めていない」ことに気付けない。ライセンス表示は
//! パスによる検索も要らない固定の1ファイルなので、`include_str!`で
//! **ビルド構成によらず必ず埋め込まれる**ほうが目的に合う。
//!
//! 生成は `./scripts/licenses.sh`。依存を足したら再生成する（CIが検査する）。

/// 本体のライセンス（MIT）。
const LICENSE_MIT: &str = include_str!("../../../LICENSE-MIT");

/// 本体のライセンス（Apache-2.0）。
const LICENSE_APACHE: &str = include_str!("../../../LICENSE-APACHE");

/// 依存のライセンス表示。`./scripts/licenses.sh` が生成する。
const THIRD_PARTY: &str = include_str!("../THIRD-PARTY-NOTICES.txt");

/// ライセンス表示の全文を組み立てる。
pub fn notices() -> String {
    format!(
        "dioryga {version}
Copyright (c) 2026 volanja

Dioryga は MIT または Apache-2.0 のいずれかを選択して利用できます。
Dioryga is dual-licensed under MIT or Apache-2.0, at your option.

================================================================================
MIT License（Dioryga 本体 / this software）
================================================================================

{mit}
================================================================================
Apache License 2.0（Dioryga 本体 / this software）
================================================================================

{apache}
{third_party}",
        version = env!("CARGO_PKG_VERSION"),
        mit = LICENSE_MIT,
        apache = LICENSE_APACHE,
        third_party = THIRD_PARTY,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 埋め込みが効いていることを確かめる。
    ///
    /// **配布物にライセンス表示が入っていない、という事故を防ぐのが目的である。**
    /// `include_str!`はコンパイル時に解決されるため、ここが通れば
    /// バイナリにも入っている。
    #[test]
    fn 全文が埋め込まれている() {
        let text = notices();

        // 本体側
        assert!(text.contains("Copyright (c) 2026 volanja"));
        assert!(text.contains("Apache License"));

        // 依存側。実際に使っている代表的なクレートを見る
        assert!(
            text.contains("axum"),
            "依存のライセンス表示が入っていません"
        );
        assert!(text.contains("sea-orm"));
    }

    /// 生成物が空でないこと。**生成に失敗して空ファイルが
    /// コミットされる**という壊れ方を検出する。
    #[test]
    fn 依存の表示が空でない() {
        assert!(
            THIRD_PARTY.len() > 10_000,
            "THIRD-PARTY-NOTICES.txt が短すぎます（{}バイト）。生成に失敗している可能性があります",
            THIRD_PARTY.len()
        );
    }
}

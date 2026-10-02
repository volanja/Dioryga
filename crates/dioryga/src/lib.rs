//! Dioryga 本体。
//!
//! バイナリ（`main.rs`）はCLIの振り分けのみを行い、実装はこのライブラリ側に置く。
//! 結合テストから参照できるようにするため。

// 翻訳ファイルはビルド時にバイナリへ埋め込む（設計書16.5）。
// 静的アセットを rust-embed で埋め込むのと同じ発想。
rust_i18n::i18n!("locales", fallback = "en");

pub mod admin;
pub mod auth;
pub mod cli;
pub mod config;
pub mod console;
pub mod container;
pub mod cost;
pub mod currency;
pub mod date;
pub mod db;
pub mod error;
pub mod import;
pub mod licenses;
pub mod repository;
pub mod sbom;
pub mod server;
pub mod setting;
pub mod telemetry;
pub mod tz;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    fn キー(yaml: &str) -> BTreeSet<String> {
        fn 集める(prefix: &str, v: &serde_yaml_ng::Value, out: &mut BTreeSet<String>) {
            match v {
                serde_yaml_ng::Value::Mapping(m) => {
                    for (k, v) in m {
                        let k = k.as_str().unwrap_or_default();
                        let path = if prefix.is_empty() {
                            k.to_owned()
                        } else {
                            format!("{prefix}.{k}")
                        };
                        集める(&path, v, out);
                    }
                }
                _ => {
                    out.insert(prefix.to_owned());
                }
            }
        }
        let mut out = BTreeSet::new();
        集める("", &serde_yaml_ng::from_str(yaml).unwrap(), &mut out);
        out
    }

    /// **日本語と英語で同じキーを持つこと**（#191）。
    ///
    /// 日本語だけ書き忘れると、既定（`fallback = "en"`）の英語が黙って出る。
    /// 画面でもコンソールでも、見た目では気づきにくい。
    #[test]
    fn 翻訳のキーが言語間でそろっている() {
        let ja = キー(include_str!("../locales/ja.yml"));
        let en = キー(include_str!("../locales/en.yml"));
        let 日本語だけ: Vec<_> = ja.difference(&en).collect();
        let 英語だけ: Vec<_> = en.difference(&ja).collect();
        assert!(
            日本語だけ.is_empty() && 英語だけ.is_empty(),
            "日本語だけ: {日本語だけ:?}\n英語だけ: {英語だけ:?}"
        );
    }
}

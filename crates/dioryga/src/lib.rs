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
pub mod db;
pub mod error;
pub mod repository;
pub mod server;
pub mod telemetry;

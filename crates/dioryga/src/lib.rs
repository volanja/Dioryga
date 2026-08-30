//! Dioryga 本体。
//!
//! バイナリ（`main.rs`）はCLIの振り分けのみを行い、実装はこのライブラリ側に置く。
//! 結合テストから参照できるようにするため。

pub mod auth;
pub mod cli;
pub mod config;
pub mod db;
pub mod error;
pub mod repository;
pub mod server;
pub mod telemetry;

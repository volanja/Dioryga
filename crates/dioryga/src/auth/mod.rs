//! 認証（設計書20章）。
//!
//! HTTP層への組み込み（ミドルウェア、ログイン・ログアウトのエンドポイント）は
//! 別のissueで扱う。ここではそれらが載る土台を提供する。

pub mod cookie;
pub mod csrf;
pub mod password;
pub mod rate_limit;
pub mod session;

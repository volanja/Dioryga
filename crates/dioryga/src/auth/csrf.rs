//! CSRFトークン（設計書20.5）。
//!
//! `SameSite=Lax` だけに頼らず、**状態変更を伴うリクエストにはトークンを要求する。**
//! Askamaのフォームにはhidden fieldとして埋め込み、htmxからのリクエストは
//! `hx-headers` でヘッダーに載せる。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

/// フォームのhidden fieldおよびヘッダーの名前。
pub const FIELD_NAME: &str = "csrf_token";
pub const HEADER_NAME: &str = "x-csrf-token";

#[derive(Debug, thiserror::Error)]
#[error("CSRFトークンの生成に失敗しました: {0}")]
pub struct CsrfError(String);

/// 新しいCSRFトークンを生成する。
pub fn generate() -> Result<String, CsrfError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| CsrfError(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// セッショントークンからCSRFトークンを導出する。
///
/// **保存しない。**`SESSION`に列を足さずに済み、セッションと必ず対応する。
///
/// 安全性の根拠：
/// - 導出元の生のセッショントークンはCookieにしか存在せず、DBには
///   SHA-256しか保存されていない。**DBを読めてもCSRFトークンは作れない**
/// - SHA-256は一方向であるため、**CSRFトークンが漏れてもセッショントークンは
///   復元できない**（フォームに埋め込む値であり、参照元ヘッダー等で漏れうる）
pub fn derive(session_raw_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"dioryga-csrf-v1:");
    hasher.update(session_raw_token.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// 送られてきたトークンが、セッションに紐づくものと一致するか。
pub fn verify(expected: &str, provided: &str) -> bool {
    super::constant_time_eq(expected, provided)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 生成されるトークンは毎回異なる() {
        assert_ne!(generate().unwrap(), generate().unwrap());
    }

    #[test]
    fn 一致すれば通る() {
        let token = generate().unwrap();
        assert!(verify(&token, &token));
    }

    #[test]
    fn 一致しなければ通らない() {
        assert!(!verify(&generate().unwrap(), &generate().unwrap()));
    }

    #[test]
    fn 導出は同じセッションなら同じ値になる() {
        assert_eq!(derive("session-token"), derive("session-token"));
    }

    #[test]
    fn 導出はセッションごとに異なる() {
        assert_ne!(derive("session-a"), derive("session-b"));
    }

    #[test]
    fn 導出値からセッショントークンは分からない() {
        let session = "very-secret-session-token";
        let csrf = derive(session);
        assert!(!csrf.contains(session));
        assert_ne!(csrf, session);
    }

    #[test]
    fn 空のトークンは常に拒否される() {
        let token = generate().unwrap();
        assert!(!verify("", ""));
        assert!(!verify(&token, ""));
        assert!(!verify("", &token));
    }
}

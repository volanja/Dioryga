//! CSRFトークン（設計書20.5）。
//!
//! `SameSite=Lax` だけに頼らず、**状態変更を伴うリクエストにはトークンを要求する。**
//! Askamaのフォームにはhidden fieldとして埋め込み、htmxからのリクエストは
//! `hx-headers` でヘッダーに載せる。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;

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
    fn 空のトークンは常に拒否される() {
        let token = generate().unwrap();
        assert!(!verify("", ""));
        assert!(!verify(&token, ""));
        assert!(!verify("", &token));
    }
}

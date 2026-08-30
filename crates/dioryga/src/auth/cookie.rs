//! セッションCookieの組み立て（設計書20.5）。

use crate::auth::session::{RawToken, COOKIE_NAME};
use crate::config::SessionConfig;

/// セッションCookieのSet-Cookie値を組み立てる。
///
/// 属性の意図（設計書20.5）：
/// - `HttpOnly`：JavaScriptから読めないようにする
/// - `SameSite=Lax`：CSRFの一次防御。ただしこれだけには頼らず、状態変更には
///   CSRFトークンも要求する
/// - `Secure`：HTTPSで動作している場合のみ付ける。**ローカルHTTP起動という
///   形態がある**ため設定で切り替える（付けてしまうとHTTPでCookieが送られない）
pub fn build(token: &RawToken, config: &SessionConfig) -> String {
    let mut cookie = format!(
        "{COOKIE_NAME}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        token.as_str(),
        config.absolute_timeout_secs
    );
    if config.cookie_secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// セッションCookieを削除するためのSet-Cookie値。
pub fn clear(config: &SessionConfig) -> String {
    let mut cookie = format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if config.cookie_secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Cookieヘッダーからセッショントークンを取り出す。
pub fn extract(header: &str) -> Option<&str> {
    header.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == COOKIE_NAME).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 設定(secure: bool) -> SessionConfig {
        SessionConfig {
            cookie_secure: secure,
            ..SessionConfig::default()
        }
    }

    #[test]
    fn 必要な属性が付く() {
        let token = RawToken::generate().unwrap();
        let cookie = build(&token, &設定(false));

        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Path=/"));
        assert!(cookie.contains(token.as_str()));
    }

    #[test]
    fn secureは設定で切り替わる() {
        let token = RawToken::generate().unwrap();
        // ローカルHTTP起動では付けない（付けるとCookieが送られない）
        assert!(!build(&token, &設定(false)).contains("Secure"));
        assert!(build(&token, &設定(true)).contains("; Secure"));
    }

    #[test]
    fn 削除用のcookieは即時失効する() {
        assert!(clear(&設定(false)).contains("Max-Age=0"));
    }

    #[test]
    fn cookieヘッダーからトークンを取り出せる() {
        let header = format!("other=1; {COOKIE_NAME}=abc123; another=2");
        assert_eq!(extract(&header), Some("abc123"));
    }

    #[test]
    fn 該当のcookieが無ければnone() {
        assert_eq!(extract("other=1; another=2"), None);
        assert_eq!(extract(""), None);
    }
}

//! 認証（設計書20章）。

pub mod authorization;
pub mod cookie;
pub mod csrf;
/// 開発専用。**`dev-autologin` 機能を有効にしたビルドにしか存在しない**（#128）。
#[cfg(feature = "dev-autologin")]
pub mod dev_autologin;
pub mod middleware;
pub mod password;
pub mod rate_limit;
pub mod session;
pub mod setup;

/// 秘密の値どうしを定数時間で比較する。
///
/// 一致しない位置で早期に返ると、比較にかかった時間から「どこまで一致していたか」が
/// 漏れる。CSRFトークンやセットアップトークンのように、推測を試みられる値の比較に使う。
pub fn constant_time_eq(expected: &str, provided: &str) -> bool {
    if expected.is_empty() || provided.is_empty() {
        return false;
    }
    if expected.len() != provided.len() {
        return false;
    }

    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(provided.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同じ値なら真() {
        assert!(constant_time_eq("abcdef", "abcdef"));
    }

    #[test]
    fn 異なる値なら偽() {
        assert!(!constant_time_eq("abcdef", "abcdeg"));
        assert!(!constant_time_eq("abcdef", "abcde"));
    }

    #[test]
    fn 空文字は常に偽() {
        assert!(!constant_time_eq("", ""));
        assert!(!constant_time_eq("abc", ""));
        assert!(!constant_time_eq("", "abc"));
    }
}

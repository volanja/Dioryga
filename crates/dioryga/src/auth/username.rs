//! ユーザー名（設計書20.1、#136）。
//!
//! **ログインIDはユーザー名。**メールアドレスは異動・社名変更・ドメイン移行で
//! 変わりうるため、ログインIDにも一意なキーにもしない（任意の連絡先として持つ）。
//!
//! # 規則
//!
//! | 項目 | 規則 |
//! |---|---|
//! | 使える文字 | ASCIIの英小文字・数字と `.` `_` `-` |
//! | 長さ | 2〜32文字 |
//! | 先頭の文字 | 英数字 |
//! | 大文字小文字 | **区別しない。**小文字にして保存し、比較する |
//!
//! 大文字小文字を区別しないのは RFC 8265（PRECIS）の UsernameCaseMapped に
//! 従う。区別すると `Hotaka` と `hotaka` が別の利用者として並ぶ。
//!
//! **ASCII以外を許さない。**全角・半角の揺れや、見た目が同じで符号の違う文字を
//! 持ち込まないため。日本語で読ませたい名前は表示名が担う。

/// ユーザー名の最短の長さ。
pub const MIN_LEN: usize = 2;
/// ユーザー名の最長の長さ。
pub const MAX_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsernameError {
    #[error("ユーザー名を入力してください")]
    Empty,

    #[error("ユーザー名は{min}〜{max}文字で入力してください")]
    Length { min: usize, max: usize },

    #[error("ユーザー名に使えるのは英小文字・数字と . _ - です")]
    InvalidChar,

    #[error("ユーザー名の先頭は英字か数字にしてください")]
    InvalidStart,
}

/// 保存・比較の形にそろえる（前後の空白を除き、小文字にする）。
///
/// **検証はしない。**ログイン時はこれだけを掛けて引く——形式の誤りを別の文言で
/// 返すと、20.6 の「存在しない場合と同じ扱い」が崩れる。
pub fn 正規化する(input: &str) -> String {
    input.trim().to_ascii_lowercase()
}

/// 登録に使えるユーザー名か確かめ、保存する形で返す。
pub fn 検証する(input: &str) -> Result<String, UsernameError> {
    let value = 正規化する(input);
    if value.is_empty() {
        return Err(UsernameError::Empty);
    }

    let length = value.chars().count();
    if !(MIN_LEN..=MAX_LEN).contains(&length) {
        return Err(UsernameError::Length {
            min: MIN_LEN,
            max: MAX_LEN,
        });
    }

    if !value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    {
        return Err(UsernameError::InvalidChar);
    }

    let 先頭 = value.chars().next().expect("空でないことは確かめた");
    if !先頭.is_ascii_alphanumeric() {
        return Err(UsernameError::InvalidStart);
    }

    Ok(value)
}

/// 任意入力のメールアドレスを、保存する形にそろえる。空なら `None`。
///
/// **値がある場合は一意**（設計書20.1）。比較をぶれさせないよう小文字にする。
pub fn 任意のメールアドレス(input: &str) -> Option<String> {
    let value = input.trim().to_lowercase();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 大文字は小文字にそろえる() {
        assert_eq!(検証する("Hotaka.Kojo").unwrap(), "hotaka.kojo");
    }

    #[test]
    fn 前後の空白は除く() {
        assert_eq!(検証する("  yarigatake  ").unwrap(), "yarigatake");
    }

    #[test]
    fn 境界の長さ() {
        assert!(検証する("ab").is_ok());
        assert!(検証する(&"a".repeat(32)).is_ok());
        assert_eq!(
            検証する("a"),
            Err(UsernameError::Length { min: 2, max: 32 })
        );
        assert_eq!(
            検証する(&"a".repeat(33)),
            Err(UsernameError::Length { min: 2, max: 32 })
        );
    }

    #[test]
    fn 使える記号() {
        assert!(検証する("hakuba.web-01_ops").is_ok());
    }

    /// **ASCII以外は許さない。**全角英字は小文字化でも半角にならず、ここで弾く。
    #[test]
    fn ascii以外は拒否する() {
        assert_eq!(検証する("槍ヶ岳"), Err(UsernameError::InvalidChar));
        assert_eq!(検証する("ｈｏｔａｋａ"), Err(UsernameError::InvalidChar));
    }

    #[test]
    fn 途中の空白と使えない記号は拒否する() {
        assert_eq!(検証する("hotaka kojo"), Err(UsernameError::InvalidChar));
        assert_eq!(検証する("hotaka@example"), Err(UsernameError::InvalidChar));
    }

    #[test]
    fn 先頭が記号なら拒否する() {
        assert_eq!(検証する(".hotaka"), Err(UsernameError::InvalidStart));
        assert_eq!(検証する("_hotaka"), Err(UsernameError::InvalidStart));
        assert!(検証する("0hotaka").is_ok());
    }

    #[test]
    fn 空は拒否する() {
        assert_eq!(検証する(""), Err(UsernameError::Empty));
        assert_eq!(検証する("   "), Err(UsernameError::Empty));
    }

    #[test]
    fn 正規化は検証しない() {
        assert_eq!(正規化する(" 槍ヶ岳 "), "槍ヶ岳");
    }

    #[test]
    fn メールアドレスは任意() {
        assert_eq!(任意のメールアドレス(""), None);
        assert_eq!(任意のメールアドレス("  "), None);
        assert_eq!(
            任意のメールアドレス(" Hotaka@Example.invalid "),
            Some("hotaka@example.invalid".to_owned())
        );
    }
}

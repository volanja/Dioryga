//! 集計通貨（設計書5.4、24.2.1）。
//!
//! `PROJECT.currency` に入れてよい値を絞る。**自由入力にしない。**金額は最小通貨
//! 単位の整数で保存し、**小数点以下の桁数は `PROJECT.currency` から決まる**ため、
//! 未知の通貨コードが入ると金額の解釈ができなくなる。

/// 対応する通貨。
///
/// 必要になった時点で足す。**桁数を確認せずに増やさないこと**（24.2.1）。
pub const SUPPORTED: &[Currency] = &[
    Currency {
        code: "JPY",
        minor_digits: 0,
    },
    Currency {
        code: "USD",
        minor_digits: 2,
    },
    Currency {
        code: "EUR",
        minor_digits: 2,
    },
];

pub const DEFAULT: &str = "JPY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Currency {
    pub code: &'static str,
    /// 小数点以下の桁数。JPYは0、USDは2（設計書24.2.1）。
    #[allow(dead_code)] // 金額を扱う画面（10章）で使う
    pub minor_digits: u32,
}

/// 対応している通貨コードか。
pub fn is_supported(code: &str) -> bool {
    SUPPORTED.iter().any(|c| c.code == code)
}

/// 対応していなければ既定へ倒す。
pub fn normalize(code: &str) -> String {
    let upper = code.trim().to_uppercase();
    if is_supported(&upper) {
        upper
    } else {
        DEFAULT.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 対応通貨だけを受け入れる() {
        assert!(is_supported("JPY"));
        assert!(is_supported("USD"));
        assert!(!is_supported("XYZ"));
        assert!(!is_supported("jpy"), "大文字小文字は呼び出し側で揃える");
    }

    #[test]
    fn 未知の通貨は既定へ倒す() {
        assert_eq!(normalize("usd"), "USD");
        assert_eq!(normalize(" JPY "), "JPY");
        assert_eq!(normalize("XYZ"), DEFAULT);
        assert_eq!(normalize(""), DEFAULT);
    }

    /// **円は小数点以下を持たない。**ここを取り違えると金額が100倍ずれる。
    #[test]
    fn 桁数が通貨ごとに定義されている() {
        let jpy = SUPPORTED.iter().find(|c| c.code == "JPY").unwrap();
        let usd = SUPPORTED.iter().find(|c| c.code == "USD").unwrap();
        assert_eq!(jpy.minor_digits, 0);
        assert_eq!(usd.minor_digits, 2);
    }
}

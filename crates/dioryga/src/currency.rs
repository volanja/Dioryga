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

fn 桁数(code: &str) -> u32 {
    SUPPORTED
        .iter()
        .find(|c| c.code == code)
        .map(|c| c.minor_digits)
        .unwrap_or(0)
}

/// 人が入力した金額を最小通貨単位の整数にする（24.2.1）。
///
/// **桁数は通貨から決まる。**`1234.56` はUSDなら `123456`、JPYなら**誤り**——
/// 円は小数点以下を持たないため、黙って切り捨てると入力の意図が失われる。
///
/// 読めなければ `None`。**黙って0に倒さない**（8.6）。
pub fn 最小単位へ(input: &str, code: &str) -> Option<i64> {
    let digits = 桁数(code);
    let v = input.trim().replace(',', "");
    if v.is_empty() {
        return None;
    }

    let (整数部, 小数部) = match v.split_once('.') {
        Some((a, b)) => (a, b),
        None => (v.as_str(), ""),
    };
    // **桁数を超える小数は受け付けない。**切り捨てると入力と保存値が食い違う
    if 小数部.len() > digits as usize {
        return None;
    }
    if 整数部.is_empty() || !整数部.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if !小数部.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let 埋めた = format!("{整数部}{小数部:0<width$}", width = digits as usize);
    埋めた.parse::<i64>().ok()
}

/// 最小通貨単位の整数を人が読む形にする。
pub fn 表示(amount: i64, code: &str) -> String {
    let digits = 桁数(code) as usize;
    if digits == 0 {
        return amount.to_string();
    }
    let 単位 = 10i64.pow(digits as u32);
    format!(
        "{}.{:0width$}",
        amount / 単位,
        (amount % 単位).abs(),
        width = digits
    )
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

    /// **桁数は通貨から決まる**（設計書24.2.1）。ここを取り違えると100倍ずれる。
    #[test]
    fn 最小単位へ変換する() {
        assert_eq!(最小単位へ("1234", "JPY"), Some(1234));
        assert_eq!(最小単位へ("1,234", "JPY"), Some(1234));
        assert_eq!(最小単位へ("1234.56", "USD"), Some(123456));
        assert_eq!(最小単位へ("1234.5", "USD"), Some(123450));
        assert_eq!(最小単位へ("1234", "USD"), Some(123400));
    }

    /// **桁数を超える小数は受け付けない。**切り捨てると入力と保存値が食い違う。
    #[test]
    fn 桁数を超える小数は拒否する() {
        assert_eq!(最小単位へ("1234.5", "JPY"), None, "円に小数は無い");
        assert_eq!(最小単位へ("1234.567", "USD"), None);
        assert_eq!(最小単位へ("", "JPY"), None);
        assert_eq!(最小単位へ("abc", "JPY"), None);
        assert_eq!(最小単位へ("-1", "JPY"), None, "負は受けない");
    }

    #[test]
    fn 表示は桁数に従う() {
        assert_eq!(表示(1234, "JPY"), "1234");
        assert_eq!(表示(123456, "USD"), "1234.56");
        assert_eq!(表示(123400, "USD"), "1234.00");
    }
}

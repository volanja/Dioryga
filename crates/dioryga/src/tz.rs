//! 表示タイムゾーン（設計書24.2.3、#210）。
//!
//! # 保存は UTC、変換は表示のときだけ
//!
//! 日時は常に UTC で保存する。**画面に出すときと「今日」を決めるときだけ、利用者の
//! タイムゾーンへ変換する。**変換をここに集め、各画面はこの関数を通す——画面ごとに
//! 書くと、UTC のまま出す箇所が必ず残る（#210 はそうして見つかった）。
//!
//! # 利用者の設定が無ければ、サーバ設定の既定
//!
//! `USER.timezone` が null の利用者と CLI には、設定ファイルの `timezone` を使う。
//!
//! # date 型は変換しない
//!
//! 取得日・予定日・期日は暦の日付であり、タイムゾーンを持たない。

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use entity::app_user;

/// IANA のタイムゾーン名を読む。**語彙外は `None`**（既定へ寄せない、Q-21）。
pub fn 読む(name: &str) -> Option<Tz> {
    name.trim().parse::<Tz>().ok()
}

/// 選べるタイムゾーンの一覧（IANA 名の辞書順）。
pub fn 一覧() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = chrono_tz::TZ_VARIANTS.iter().map(|t| t.name()).collect();
    names.sort_unstable();
    names
}

/// 利用者のタイムゾーン。**未設定、または読めない値なら既定**（設定ファイルの値）。
///
/// 読めない値は画面が拒否するため入らないはずだが、DB を直接直された場合に
/// 画面を落とさない。
pub fn 利用者の(user: &app_user::Model, 既定: Tz) -> Tz {
    user.timezone.as_deref().and_then(読む).unwrap_or(既定)
}

/// 日時を利用者のタイムゾーンの日付にする。
pub fn 日付(at: DateTime<Utc>, tz: Tz) -> NaiveDate {
    at.with_timezone(&tz).date_naive()
}

/// 日時を `2026-04-01 09:30` の形にする。
pub fn 日時(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()
}

/// 利用者のタイムゾーンでの今日。**UTC の日付で判定すると、日本では朝9時まで
/// 前日扱いになる。**
pub fn 今日(tz: Tz) -> NaiveDate {
    日付(Utc::now(), tz)
}

/// 日付を、利用者のタイムゾーンのその日の0時として UTC の日時にする
/// （取込画面の基準日など）。
///
/// 夏時間の切り替えで0時が存在しない日は、その日の最初に存在する時刻を使う。
pub fn その日の始まり(date: NaiveDate, tz: Tz) -> DateTime<Utc> {
    let 零時 = date.and_hms_opt(0, 0, 0).expect("0時は常に作れる");
    match tz.from_local_datetime(&零時) {
        chrono::LocalResult::Single(t) => t.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(早い, _) => 早い.with_timezone(&Utc),
        // 0時が飛ばされる日（夏時間の開始が0時の地域）は1時間後から始まる
        chrono::LocalResult::None => tz
            .from_local_datetime(&(零時 + chrono::Duration::hours(1)))
            .earliest()
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&零時)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// **同じ日時が、タイムゾーンによって別の日付になる。**
    #[test]
    fn 日付はタイムゾーンで変わる() {
        let at = utc("2026-03-31T15:00:00Z");
        assert_eq!(日付(at, chrono_tz::Asia::Tokyo).to_string(), "2026-04-01");
        assert_eq!(日付(at, chrono_tz::UTC).to_string(), "2026-03-31");
        assert_eq!(日時(at, chrono_tz::Asia::Tokyo), "2026-04-01 00:00");
    }

    #[test]
    fn その日の始まりは現地の0時() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert_eq!(
            その日の始まり(d, chrono_tz::Asia::Tokyo),
            utc("2026-03-31T15:00:00Z")
        );
    }

    #[test]
    fn 語彙外のタイムゾーンは読まない() {
        assert!(読む("Asia/Tokyo").is_some());
        assert!(読む("Asia/Tokio").is_none());
        assert!(読む("").is_none());
        assert!(一覧().contains(&"Asia/Tokyo"));
    }
}

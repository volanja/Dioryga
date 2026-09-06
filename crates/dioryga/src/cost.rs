//! 費用の按分と年間コストの計算（設計書10.3、24.2.2）。
//!
//! # 値は保存せず、都度計算する（不変条件2）
//!
//! 年間コストも簿価も保存しない。**保存すると、元になった契約や資産が直された
//! ときに古い数字が残る。**
//!
//! # 按分の丸め規則（24.2.2）
//!
//! > 按分は最小通貨単位で切り捨て、生じた端数はすべて最終期間に加算する。
//!
//! **これにより各期間の按分額の合計は、必ず元の金額と一致する。**規則がないと
//! 「12ヶ月分の合計が契約額と一致しない」という状態が生じる。
//!
//! **一致することを事後条件として常時チェックする**（24.2.2）。破れた場合は
//! 計算結果を返さず、按分できなかったものとして扱う。
//!
//! # 計算できない行は合算から外して列挙する（10.3）
//!
//! 期間が0以下、耐用年数が0以下、金額が負、定率法（Q-12）、プロジェクトと
//! 異なる通貨（Q-6）。**黙って除外しない**——金額が小さいのが実態なのか
//! データ不備なのかを、利用者が判別できる必要がある。

use chrono::{Datelike, NaiveDate};

/// 定額法。
pub const STRAIGHT_LINE: &str = "straight_line";
/// 定率法。**v1では計算しない**（Q-12）。
pub const DECLINING_BALANCE: &str = "declining_balance";

pub const DEPRECIATION_METHODS: &[&str] = &[STRAIGHT_LINE, DECLINING_BALANCE];
pub const BILLING_CYCLES: &[&str] = &["Monthly", "Annual"];

/// 計算できなかった理由（10.3）。**画面にそのまま出す。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum 除外の理由 {
    /// 定率法。償却率テーブルが未実装（Q-12）。
    定率法,
    /// プロジェクトと異なる通貨（Q-6）。
    通貨違い,
    /// `end_date` が `start_date` より前、など。
    期間が不正,
    /// `useful_life_years <= 0`。
    耐用年数が不正,
    /// 入力ミス。
    金額が負,
}

impl 除外の理由 {
    pub fn key(self) -> &'static str {
        match self {
            Self::定率法 => "costs.excluded_declining",
            Self::通貨違い => "costs.excluded_currency",
            Self::期間が不正 => "costs.excluded_period",
            Self::耐用年数が不正 => "costs.excluded_life",
            Self::金額が負 => "costs.excluded_negative",
        }
    }
}

// ---------------------------------------------------------------------------
// 按分
// ---------------------------------------------------------------------------

/// 金額を期間数で按分する（24.2.2）。
///
/// **切り捨てた端数はすべて最終期間に加算する。**返した配列の合計は必ず
/// `amount` と一致し、これを事後条件として検査している。
///
/// `periods` が0以下、`amount` が負なら `None`。
pub fn 按分する(amount: i64, periods: i64) -> Option<Vec<i64>> {
    if periods <= 0 || amount < 0 {
        return None;
    }

    let 一期あたり = amount / periods;
    let mut out = vec![一期あたり; periods as usize];
    // **端数はすべて最終期間へ。**分散させると期間ごとに1円ずれる行が生まれ、
    // どこに寄せたのかが説明できなくなる
    let 端数 = amount - 一期あたり * periods;
    if let Some(last) = out.last_mut() {
        *last += 端数;
    }

    // **事後条件**（24.2.2）。ここが破れたら計算結果を出さない
    debug_assert_eq!(out.iter().sum::<i64>(), amount);
    if out.iter().sum::<i64>() != amount {
        return None;
    }
    Some(out)
}

/// 期間に含まれる月数（両端を含む）。
///
/// **月単位で数える。**10.3が「年割りではなく月割り」としているため、日数では
/// なく月をまたいだ回数で数える。
pub fn 月数(start: NaiveDate, end: NaiveDate) -> i64 {
    let 年 = end.year() as i64 - start.year() as i64;
    let 月 = end.month() as i64 - start.month() as i64;
    年 * 12 + 月 + 1
}

/// その年に含まれる月のうち、期間と重なるものの数。
pub fn その年の月数(start: NaiveDate, end: NaiveDate, year: i32) -> i64 {
    let 年初 = NaiveDate::from_ymd_opt(year, 1, 1).expect("年初");
    let 年末 = NaiveDate::from_ymd_opt(year, 12, 31).expect("年末");

    let 開始 = start.max(年初);
    let 終了 = end.min(年末);
    if 開始 > 終了 {
        return 0;
    }
    月数(開始, 終了)
}

/// 契約・定期費用のその年の費用（10.3）。
///
/// **月単位で按分してから、該当年に含まれる月数分を合算する。**
/// `billing_cycle="Annual"` も12ヶ月に均等分割してから按分する——保守契約費用と
/// 同じ按分方式に揃えるため。
pub fn 期間費用のその年の額(
    amount: i64,
    start: NaiveDate,
    end: NaiveDate,
    year: i32,
) -> Result<i64, 除外の理由> {
    if amount < 0 {
        return Err(除外の理由::金額が負);
    }
    if end < start {
        return Err(除外の理由::期間が不正);
    }

    let 全月数 = 月数(start, end);
    let 按分 = 按分する(amount, 全月数).ok_or(除外の理由::期間が不正)?;

    // 何ヶ月目から何ヶ月目がその年か
    let 年初 = NaiveDate::from_ymd_opt(year, 1, 1).expect("年初");
    let 年末 = NaiveDate::from_ymd_opt(year, 12, 31).expect("年末");
    let 開始 = start.max(年初);
    let 終了 = end.min(年末);
    if 開始 > 終了 {
        return Ok(0);
    }

    let 先頭 = (月数(start, 開始) - 1) as usize;
    let 本数 = その年の月数(start, end, year) as usize;
    Ok(按分.iter().skip(先頭).take(本数).sum())
}

/// 固定資産のその年の減価償却費（10.3）。
///
/// **定額法だけを計算する。**定率法は償却率テーブルが要るため、計算せず
/// 除外の理由を返す（Q-12）。
pub fn 減価償却費(
    acquisition_cost: i64,
    method: &str,
    useful_life_years: i32,
    acquisition_date: NaiveDate,
    year: i32,
) -> Result<i64, 除外の理由> {
    if method != STRAIGHT_LINE {
        return Err(除外の理由::定率法);
    }
    if acquisition_cost < 0 {
        return Err(除外の理由::金額が負);
    }
    if useful_life_years <= 0 {
        return Err(除外の理由::耐用年数が不正);
    }

    // **取得年から耐用年数のあいだだけ計上する。**償却が終わった資産を
    // 毎年計上し続けると、年間コストが実態から離れる
    let 開始年 = acquisition_date.year();
    let 終了年 = 開始年 + useful_life_years - 1;
    if year < 開始年 || year > 終了年 {
        return Ok(0);
    }

    // **年で按分する**（24.2.2の規則は月割りだけでなく定額法にも適用する）
    let 按分 =
        按分する(acquisition_cost, useful_life_years as i64).ok_or(除外の理由::耐用年数が不正)?;
    Ok(按分[(year - 開始年) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 日(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// **合計が元の金額と必ず一致すること**（設計書24.2.2）。
    ///
    /// 規則がないと「12ヶ月分の合計が契約額と一致しない」という状態が生じる。
    #[test]
    fn 按分の合計は元の金額と一致する() {
        for (amount, periods) in [
            (100_000, 12),
            (100_001, 12),
            (1, 12),
            (0, 12),
            (999_999_999, 7),
        ] {
            let 按分 = 按分する(amount, periods).unwrap();
            assert_eq!(按分.iter().sum::<i64>(), amount, "{amount} / {periods}");
            assert_eq!(按分.len(), periods as usize);
        }
    }

    /// **端数はすべて最終期間に加算すること**（24.2.2）。
    #[test]
    fn 端数は最終期間へ寄せる() {
        // 100 を 3 で割ると 33 余り 1
        let 按分 = 按分する(100, 3).unwrap();
        assert_eq!(按分, vec![33, 33, 34]);
    }

    #[test]
    fn 按分できない入力はnone() {
        assert!(按分する(100, 0).is_none());
        assert!(按分する(100, -1).is_none());
        assert!(按分する(-1, 12).is_none());
    }

    /// 月数は両端を含むこと。
    #[test]
    fn 月数は両端を含む() {
        assert_eq!(月数(日(2026, 4, 1), 日(2027, 3, 31)), 12);
        assert_eq!(月数(日(2026, 4, 1), 日(2026, 4, 30)), 1);
        assert_eq!(月数(日(2026, 1, 1), 日(2026, 12, 31)), 12);
    }

    /// **年をまたぐ契約が、年ごとに分かれること**（設計書10.3）。
    ///
    /// 4月開始の12ヶ月契約なら、初年度に9ヶ月、翌年度に3ヶ月。
    #[test]
    fn 年をまたぐ契約は月数で分かれる() {
        let (start, end) = (日(2026, 4, 1), 日(2027, 3, 31));
        let a = 期間費用のその年の額(120_000, start, end, 2026).unwrap();
        let b = 期間費用のその年の額(120_000, start, end, 2027).unwrap();

        assert_eq!(a, 90_000, "2026年は9ヶ月");
        assert_eq!(b, 30_000, "2027年は3ヶ月");
        // **合計は契約額と一致する**
        assert_eq!(a + b, 120_000);
    }

    /// 端数が出ても、年ごとの合計は契約額と一致すること。
    #[test]
    fn 端数が出ても年の合計は契約額と一致する() {
        let (start, end) = (日(2026, 4, 1), 日(2027, 3, 31));
        let amount = 100_000;
        let a = 期間費用のその年の額(amount, start, end, 2026).unwrap();
        let b = 期間費用のその年の額(amount, start, end, 2027).unwrap();
        assert_eq!(a + b, amount);
    }

    #[test]
    fn 期間外の年は零になる() {
        let (start, end) = (日(2026, 4, 1), 日(2027, 3, 31));
        assert_eq!(期間費用のその年の額(120_000, start, end, 2025).unwrap(), 0);
        assert_eq!(期間費用のその年の額(120_000, start, end, 2028).unwrap(), 0);
    }

    #[test]
    fn 期間が逆なら理由を返す() {
        let r = 期間費用のその年の額(1000, 日(2027, 1, 1), 日(2026, 1, 1), 2026);
        assert_eq!(r, Err(除外の理由::期間が不正));
    }

    /// **定額法は取得年から耐用年数ぶんだけ計上すること**（設計書10.3）。
    #[test]
    fn 定額法は耐用年数のあいだ計上する() {
        let 取得 = 日(2026, 4, 1);
        let mut 合計 = 0;
        for year in 2026..=2030 {
            合計 += 減価償却費(500_000, STRAIGHT_LINE, 5, 取得, year).unwrap();
        }
        assert_eq!(合計, 500_000, "耐用年数ぶんの合計が取得価格と一致しない");

        // 償却が終わったら計上しない
        assert_eq!(
            減価償却費(500_000, STRAIGHT_LINE, 5, 取得, 2031).unwrap(),
            0
        );
        assert_eq!(
            減価償却費(500_000, STRAIGHT_LINE, 5, 取得, 2025).unwrap(),
            0
        );
    }

    /// **定率法は計算せず、理由を返すこと**（Q-12、設計書10.3）。
    ///
    /// 黙って0にすると、年間コストが実態より小さく出る。
    #[test]
    fn 定率法は計算しない() {
        let r = 減価償却費(500_000, DECLINING_BALANCE, 5, 日(2026, 4, 1), 2026);
        assert_eq!(r, Err(除外の理由::定率法));
    }

    #[test]
    fn 耐用年数が零以下なら理由を返す() {
        let r = 減価償却費(500_000, STRAIGHT_LINE, 0, 日(2026, 4, 1), 2026);
        assert_eq!(r, Err(除外の理由::耐用年数が不正));
    }
}

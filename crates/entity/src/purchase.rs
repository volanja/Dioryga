//! 購入の記録（設計書10.2）。
//!
//! **品目ごとに1行。発注を表すテーブルは持たない。**発注番号は自由入力で重複して
//! よく、1つの注文で10台買えば同じ番号の行が10本並ぶ。注文単位の合計は
//! `order_number` で寄せれば出る。
//!
//! **数量と通貨を持たない。**台帳が数えるのは実体（`DEVICE` / `PART_INSTANCE`）で
//! あり、金額は `PROJECT.currency` で表す（5.4）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "purchase")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Device / PartInstance / SoftwareInstance。多態的参照のため外部キーを持てない。
    pub item_type: String,
    pub item_id: i32,
    /// **自由入力で、重複してよい。**
    pub order_number: Option<String>,
    /// 現品を受け取った日。**償却の開始日はこちら**（発注日は持たない）。
    pub acquired_on: Option<Date>,
    /// その品目1つの金額。**最小通貨単位の整数**（24.2.1）。
    pub amount: i64,
    /// 買った相手。**`VENDOR` を参照しない**——代理店・商社から買うのが普通で、
    /// カタログのベンダーに販売店が混ざる。
    pub supplier: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

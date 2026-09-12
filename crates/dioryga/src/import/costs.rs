//! 費用の取込（設計書23.5、10.2、10.3、24.2.1、24.2.2）。
//!
//! `PURCHASE_ORDER`＋明細、`FIXED_ASSET`、`MAINTENANCE_CONTRACT`＋明細を扱う。
//!
//! # 多態的参照は「型＋自然キー」で書く（23.5）
//!
//! ```csv
//! item_type,item_hostname,item_serial_number,acquisition_cost,...
//! Device,web01,,1200000,...
//! PartInstance,,SN-DIMM-0007,48000,...
//! ```
//!
//! **見る列は `item_type` で決まる。**`Device` は23.2の解決順序、`PartInstance`
//! はシリアル番号（ベンダー＋シリアルで一意、23.5）で引く。`item_id` はDBの
//! 内部IDであり、人が書くファイルには現れない。
//!
//! # 金額は画面と同じ経路で解釈する（24.2.1）
//!
//! [`crate::currency::最小単位へ`] を使う。**桁数はプロジェクトの通貨から
//! 決まる**ため、`1234.56` はUSDなら `123456`、JPYなら誤りである。
//! **取込専用の解釈を持たない**——画面と取込で金額の読み方が違うと、
//! 同じ数字を入れたのに保存値が変わる。
//!
//! # 按分できない入力は取り込まない（24.2.2）
//!
//! 期間が0以下・耐用年数が0以下・金額が負の行は**エラーにする。**24.2.2が
//! 「按分額の合計は必ず元の金額と一致する」を事後条件として要求しており、
//! 按分できない行を入れると10.3の集計がその行を黙って落とすことになる。
//!
//! # `as_of` を使わない
//!
//! **発注も固定資産も保守契約も履歴ではなく、日付を自分の列として持つ**
//! （10.2、10.3）。契約期間や取得日は事実として行に入っており、取込の
//! 基準時刻（23.1）で上書きするものではない。
//!
//! # 定率法は値を入れるだけ（10.3）
//!
//! `declining_balance` の按分はv1では行わない。**合算から外して画面に列挙する
//! 側**であり、取込は `depreciation_method` の値を記録するだけである。

use std::collections::HashMap;

use chrono::{NaiveDate, Utc};
use entity::{
    device, fixed_asset, maintenance_contract, maintenance_contract_item, part_instance,
    purchase_order, purchase_order_item, vendor,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use super::placement::{
    このプロジェクトの機器, 機器キー, 空ならnone, 解決する機器, 読み取る
};
use super::{Entry, ImportError, Outcome, Report};
use crate::cost::DEPRECIATION_METHODS;
use crate::currency;
use crate::repository::AuditedTx;

const DEVICE: &str = "Device";
const PART_INSTANCE: &str = "PartInstance";

// ---------------------------------------------------------------------------
// 行の定義
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct PurchaseOrderRow {
    pub order_number: String,
    pub order_date: String,
    pub vendor: String,
    pub item_type: String,
    #[serde(default)]
    pub item_hostname: String,
    #[serde(default)]
    pub item_serial_number: String,
    pub quantity: String,
    pub unit_price: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixedAssetRow {
    pub item_type: String,
    #[serde(default)]
    pub item_hostname: String,
    #[serde(default)]
    pub item_serial_number: String,
    pub acquisition_cost: String,
    pub depreciation_method: String,
    pub useful_life_years: String,
    pub acquisition_date: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MaintenanceContractRow {
    pub contract_number: String,
    pub vendor: String,
    pub start_date: String,
    pub end_date: String,
    pub amount: String,
    #[serde(default)]
    pub quote_contact: String,
    #[serde(default)]
    pub failure_contact: String,
    /// **1契約が複数品目をカバーする**ため、品目ごとに1行書く。
    pub item_type: String,
    #[serde(default)]
    pub item_hostname: String,
    #[serde(default)]
    pub item_serial_number: String,
}

pub fn parse_purchase_orders(source: &str) -> Result<Vec<PurchaseOrderRow>, ImportError> {
    読み取る(source)
}
pub fn parse_fixed_assets(source: &str) -> Result<Vec<FixedAssetRow>, ImportError> {
    読み取る(source)
}
pub fn parse_maintenance_contracts(
    source: &str,
) -> Result<Vec<MaintenanceContractRow>, ImportError> {
    読み取る(source)
}

// ---------------------------------------------------------------------------
// 発注（10.2）
// ---------------------------------------------------------------------------

pub async fn 発注を取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[PurchaseOrderRow],
    currency_code: &str,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let 対象 = 対象の索引(tx, project_id).await?;
    let now = Utc::now();

    for row in rows {
        let number = row.order_number.trim().to_owned();
        let target = format!("PURCHASE_ORDER {number}");

        if number.is_empty() {
            report.push(Entry::new(Outcome::Error, target, "order_number が空です"));
            continue;
        }

        let Some(order_date) = 日付(&row.order_date) else {
            report.push(Entry::new(
                Outcome::Error,
                target,
                format!("order_date「{}」を日付として読めません", row.order_date),
            ));
            continue;
        };

        let vendor_id = match ベンダーを引く(tx, &row.vendor).await? {
            Ok(id) => id,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let quantity = match row.quantity.trim().parse::<i32>() {
            Ok(q) if q > 0 => q,
            _ => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    format!("quantity「{}」は1以上の整数で書いてください", row.quantity),
                ));
                continue;
            }
        };

        let Some(unit_price) = currency::最小単位へ(&row.unit_price, currency_code) else {
            report.push(Entry::new(
                Outcome::Error,
                target,
                金額の誤り("unit_price", &row.unit_price, currency_code),
            ));
            continue;
        };

        let (item_type, item_id) = match 品目を引く(
            tx,
            &対象,
            &row.item_type,
            &row.item_hostname,
            &row.item_serial_number,
        )
        .await?
        {
            Ok(v) => v,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        // 発注そのものは `order_number` で突合する。**明細は発注と品目の組**
        let po = match purchase_order::Entity::find()
            .filter(purchase_order::Column::OrderNumber.eq(&number))
            .one(tx.reader())
            .await?
        {
            Some(既存) => 既存,
            None => {
                tx.insert(purchase_order::ActiveModel {
                    order_number: Set(number.clone()),
                    order_date: Set(order_date),
                    vendor_id: Set(vendor_id),
                    currency: Set(currency_code.to_owned()),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?
            }
        };

        let 明細 = purchase_order_item::Entity::find()
            .filter(purchase_order_item::Column::PurchaseOrderId.eq(po.id))
            .filter(purchase_order_item::Column::ItemType.eq(&item_type))
            .filter(purchase_order_item::Column::ItemId.eq(item_id))
            .one(tx.reader())
            .await?;

        let 明細の表示 = format!("{target} {item_type} {}", 品目名(row));
        match 明細 {
            Some(既存) if 既存.quantity == quantity && 既存.unit_price == unit_price => {
                report.push(Entry::new(Outcome::Unchanged, 明細の表示, ""));
            }
            Some(既存) => {
                let mut active: purchase_order_item::ActiveModel = 既存.clone().into();
                active.quantity = Set(quantity);
                active.unit_price = Set(unit_price);
                active.updated_at = Set(now);
                tx.update(&既存, active).await?;
                report.push(Entry::new(Outcome::Updated, 明細の表示, ""));
            }
            None => {
                tx.insert(purchase_order_item::ActiveModel {
                    purchase_order_id: Set(po.id),
                    item_type: Set(item_type),
                    item_id: Set(item_id),
                    quantity: Set(quantity),
                    unit_price: Set(unit_price),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?;
                report.push(Entry::new(Outcome::Created, 明細の表示, ""));
            }
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// 固定資産（10.3、24.2.2）
// ---------------------------------------------------------------------------

pub async fn 固定資産を取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[FixedAssetRow],
    currency_code: &str,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let 対象 = 対象の索引(tx, project_id).await?;
    let now = Utc::now();

    for row in rows {
        let target = format!("FIXED_ASSET {} {}", row.item_type.trim(), 資産の品目名(row));

        let Some(acquisition_date) = 日付(&row.acquisition_date) else {
            report.push(Entry::new(
                Outcome::Error,
                target,
                format!(
                    "acquisition_date「{}」を日付として読めません",
                    row.acquisition_date
                ),
            ));
            continue;
        };

        let Some(acquisition_cost) =
            currency::最小単位へ(&row.acquisition_cost, currency_code)
        else {
            report.push(Entry::new(
                Outcome::Error,
                target,
                金額の誤り("acquisition_cost", &row.acquisition_cost, currency_code),
            ));
            continue;
        };

        // **耐用年数が0以下は按分できない**（24.2.2）
        let useful_life_years = match row.useful_life_years.trim().parse::<i32>() {
            Ok(y) if y > 0 => y,
            _ => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    format!(
                        "useful_life_years「{}」は1以上で書いてください。0以下では按分できません",
                        row.useful_life_years
                    ),
                ));
                continue;
            }
        };

        let method = row.depreciation_method.trim();
        if !DEPRECIATION_METHODS.contains(&method) {
            report.push(Entry::new(
                Outcome::Error,
                target,
                format!(
                    "depreciation_method「{method}」は使えません（{}）",
                    DEPRECIATION_METHODS.join(" / ")
                ),
            ));
            continue;
        }

        let (item_type, item_id) = match 品目を引く(
            tx,
            &対象,
            &row.item_type,
            &row.item_hostname,
            &row.item_serial_number,
        )
        .await?
        {
            Ok(v) => v,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        // **1つの品目につき1件。**同じ機器に2件あると10.3が二重に数える
        let 既存 = fixed_asset::Entity::find()
            .filter(fixed_asset::Column::ItemType.eq(&item_type))
            .filter(fixed_asset::Column::ItemId.eq(item_id))
            .one(tx.reader())
            .await?;

        match 既存 {
            Some(a)
                if a.acquisition_cost == acquisition_cost
                    && a.depreciation_method == method
                    && a.useful_life_years == useful_life_years
                    && a.acquisition_date == acquisition_date =>
            {
                report.push(Entry::new(Outcome::Unchanged, target, ""));
            }
            Some(a) => {
                let mut active: fixed_asset::ActiveModel = a.clone().into();
                active.acquisition_cost = Set(acquisition_cost);
                active.depreciation_method = Set(method.to_owned());
                active.useful_life_years = Set(useful_life_years);
                active.acquisition_date = Set(acquisition_date);
                active.updated_at = Set(now);
                tx.update(&a, active).await?;
                report.push(Entry::new(Outcome::Updated, target, ""));
            }
            None => {
                tx.insert(fixed_asset::ActiveModel {
                    item_type: Set(item_type),
                    item_id: Set(item_id),
                    acquisition_cost: Set(acquisition_cost),
                    depreciation_method: Set(method.to_owned()),
                    useful_life_years: Set(useful_life_years),
                    acquisition_date: Set(acquisition_date),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?;
                report.push(Entry::new(Outcome::Created, target, ""));
            }
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// 保守契約（10.2、24.2.2）
// ---------------------------------------------------------------------------

pub async fn 保守契約を取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[MaintenanceContractRow],
    currency_code: &str,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let 対象 = 対象の索引(tx, project_id).await?;
    let now = Utc::now();
    // 同じ契約が複数行に現れる。**契約の内容が行ごとに食い違っていないか見る**
    let mut 契約の内容: HashMap<String, (NaiveDate, NaiveDate, i64)> = HashMap::new();

    for row in rows {
        let number = row.contract_number.trim().to_owned();
        let target = format!("MAINTENANCE_CONTRACT {number}");

        if number.is_empty() {
            report.push(Entry::new(
                Outcome::Error,
                target,
                "contract_number が空です",
            ));
            continue;
        }

        let (Some(start_date), Some(end_date)) = (日付(&row.start_date), 日付(&row.end_date))
        else {
            report.push(Entry::new(
                Outcome::Error,
                target,
                "start_date / end_date を日付として読めません",
            ));
            continue;
        };
        // **期間が0以下では按分できない**（24.2.2）
        if end_date < start_date {
            report.push(Entry::new(
                Outcome::Error,
                target,
                format!("end_date（{end_date}）が start_date（{start_date}）より前です"),
            ));
            continue;
        }

        let Some(amount) = currency::最小単位へ(&row.amount, currency_code) else {
            report.push(Entry::new(
                Outcome::Error,
                target,
                金額の誤り("amount", &row.amount, currency_code),
            ));
            continue;
        };

        // **同じ契約番号で内容が食い違う行を通さない。**どちらが正しいか決められない
        match 契約の内容.get(&number) {
            Some((s, e, a)) if (*s, *e, *a) != (start_date, end_date, amount) => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    "同じ contract_number の行で、期間か金額が食い違っています",
                ));
                continue;
            }
            _ => {
                契約の内容.insert(number.clone(), (start_date, end_date, amount));
            }
        }

        let vendor_id = match ベンダーを引く(tx, &row.vendor).await? {
            Ok(id) => id,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let (item_type, item_id) = match 品目を引く(
            tx,
            &対象,
            &row.item_type,
            &row.item_hostname,
            &row.item_serial_number,
        )
        .await?
        {
            Ok(v) => v,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let 契約 = match maintenance_contract::Entity::find()
            .filter(maintenance_contract::Column::ContractNumber.eq(&number))
            .one(tx.reader())
            .await?
        {
            Some(c) => {
                let 同じ = c.start_date == start_date
                    && c.end_date == end_date
                    && c.amount == amount
                    && c.vendor_id == vendor_id
                    && c.quote_contact == row.quote_contact.trim()
                    && c.failure_contact == row.failure_contact.trim();
                if !同じ {
                    let mut active: maintenance_contract::ActiveModel = c.clone().into();
                    active.vendor_id = Set(vendor_id);
                    active.start_date = Set(start_date);
                    active.end_date = Set(end_date);
                    active.amount = Set(amount);
                    active.quote_contact = Set(row.quote_contact.trim().to_owned());
                    active.failure_contact = Set(row.failure_contact.trim().to_owned());
                    active.updated_at = Set(now);
                    tx.update(&c, active).await?;
                    report.push(Entry::new(Outcome::Updated, target.clone(), ""));
                }
                c
            }
            None => {
                let c = tx
                    .insert(maintenance_contract::ActiveModel {
                        contract_number: Set(number.clone()),
                        vendor_id: Set(vendor_id),
                        start_date: Set(start_date),
                        end_date: Set(end_date),
                        amount: Set(amount),
                        quote_contact: Set(row.quote_contact.trim().to_owned()),
                        failure_contact: Set(row.failure_contact.trim().to_owned()),
                        purchase_order_id: Set(None),
                        created_at: Set(now),
                        updated_at: Set(now),
                        ..Default::default()
                    })
                    .await?;
                report.push(Entry::new(Outcome::Created, target.clone(), ""));
                c
            }
        };

        // **同じ契約に同じ品目を二重に付けない**
        let 明細 = maintenance_contract_item::Entity::find()
            .filter(maintenance_contract_item::Column::MaintenanceContractId.eq(契約.id))
            .filter(maintenance_contract_item::Column::ItemType.eq(&item_type))
            .filter(maintenance_contract_item::Column::ItemId.eq(item_id))
            .one(tx.reader())
            .await?;

        let 明細の表示 = format!("{target} {item_type} {}", 契約の品目名(row));
        if 明細.is_some() {
            report.push(Entry::new(Outcome::Unchanged, 明細の表示, ""));
            continue;
        }
        tx.insert(maintenance_contract_item::ActiveModel {
            maintenance_contract_id: Set(契約.id),
            item_type: Set(item_type),
            item_id: Set(item_id),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .await?;
        report.push(Entry::new(Outcome::Created, 明細の表示, ""));
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// 参照の解決
// ---------------------------------------------------------------------------

/// 品目として指せるもの（23.5）。**型ごとに見る列が変わる。**
struct 対象の索引 {
    機器: Vec<device::Model>,
    部品: Vec<part_instance::Model>,
}

async fn 対象の索引(tx: &AuditedTx, project_id: i32) -> Result<対象の索引, ImportError> {
    let 機器 = このプロジェクトの機器(tx.reader(), project_id).await?;
    let 部品 = super::parts::このプロジェクトに関わった部品(tx.reader(), &機器).await?;
    Ok(対象の索引 { 機器, 部品 })
}

/// `item_type` に応じて自然キーから品目を引く（23.5）。
///
/// **`Device` は23.2の解決順序、`PartInstance` はシリアル番号。**型ごとに
/// 取込の識別方式を作り直さずに済むよう、既にあるものを通す。
async fn 品目を引く(
    tx: &AuditedTx,
    対象: &対象の索引,
    item_type: &str,
    hostname: &str,
    serial: &str,
) -> Result<Result<(String, i32), String>, ImportError> {
    match item_type.trim() {
        DEVICE => {
            let key = 機器キー {
                hostname: hostname.to_owned(),
                serial_number: serial.to_owned(),
                ..Default::default()
            };
            Ok(match 解決する機器(tx.reader(), &対象.機器, &key).await? {
                Ok(d) => Ok((DEVICE.to_owned(), d.id)),
                Err(理由) => Err(理由),
            })
        }
        PART_INSTANCE => {
            let Some(serial) = 空ならnone(serial) else {
                return Ok(Err(
                    "item_type=PartInstance には item_serial_number が要ります".to_owned(),
                ));
            };
            let 一致: Vec<&part_instance::Model> = 対象
                .部品
                .iter()
                .filter(|p| p.serial_number.as_deref() == Some(serial))
                .collect();
            Ok(match 一致.len() {
                1 => Ok((PART_INSTANCE.to_owned(), 一致[0].id)),
                0 => Err(format!(
                    "シリアル「{serial}」の部品が、このプロジェクトの機器に載ったことがありません"
                )),
                n => Err(format!(
                    "シリアル「{serial}」の部品が{n}件あります。突合できません"
                )),
            })
        }
        // **ソフトウェアは対象外。**`SOFTWARE_INSTANCE` を指す発注もありうるが、
        // 9章の取込はv1のスコープ外であり、指す先が入っていない
        other => Ok(Err(format!(
            "item_type「{other}」は扱えません。Device / PartInstance のいずれかです"
        ))),
    }
}

async fn ベンダーを引く(
    tx: &AuditedTx,
    name: &str,
) -> Result<Result<i32, String>, ImportError> {
    let name = name.trim();
    let Some(v) = vendor::Entity::find()
        .filter(vendor::Column::Name.eq(name))
        .one(tx.reader())
        .await?
    else {
        return Ok(Err(format!("ベンダー「{name}」が見つかりません")));
    };
    // **統合で吸収されたベンダーは統合先へ寄せる**（23.9.4）
    Ok(Ok(v.merged_into_vendor_id.unwrap_or(v.id)))
}

fn 日付(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
}

/// 金額が読めなかった理由。**桁数は通貨から決まる**（24.2.1）ので、それを示す。
fn 金額の誤り(列: &str, value: &str, code: &str) -> String {
    format!(
        "{列}「{value}」を金額として読めません。{code} は小数点以下{}桁です",
        currency::SUPPORTED
            .iter()
            .find(|c| c.code == code)
            .map(|c| c.minor_digits)
            .unwrap_or(0)
    )
}

fn 品目名(row: &PurchaseOrderRow) -> String {
    表示する品目(&row.item_hostname, &row.item_serial_number)
}
fn 資産の品目名(row: &FixedAssetRow) -> String {
    表示する品目(&row.item_hostname, &row.item_serial_number)
}
fn 契約の品目名(row: &MaintenanceContractRow) -> String {
    表示する品目(&row.item_hostname, &row.item_serial_number)
}

fn 表示する品目(hostname: &str, serial: &str) -> String {
    match (空ならnone(hostname), 空ならnone(serial)) {
        (Some(h), _) => h.to_owned(),
        (None, Some(s)) => s.to_owned(),
        (None, None) => "(品目なし)".to_owned(),
    }
}

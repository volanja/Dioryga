//! QCD（費用・マイルストーン）のスキーマの結合テスト（設計書10章）。
//!
//! 画面はまだ無いため、**マイグレーションが両DBで通ること**と、**24.2.1で
//! 決めた金額の持ち方が実際に成立すること**を確かめる。丸め誤差は入った後では
//! 気付きにくく、10.3の年間コストダッシュボードで初めて表面化するため、
//! ここで固定しておく。

mod support;

use chrono::{NaiveDate, Utc};
use entity::{
    app_user, device, fixed_asset, maintenance_contract, milestone, project, purchase,
    recurring_cost, vendor,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

// ---------------------------------------------------------------------------
// 金額（設計書24.2.1）
// ---------------------------------------------------------------------------

/// **金額が整数のまま往復すること**（設計書24.2.1）。
///
/// SQLiteに `DECIMAL` は無く、`REAL` に落とすと丸め誤差が出る。JPYは
/// 小数点以下を持たないため、円単位の整数がそのまま最小通貨単位になる。
async fn 金額は整数のまま往復する(db: &DatabaseConnection) {
    // 1台 1,234,567円
    let p = 購入(db, 1, Some("PO-0001"), 1_234_567).await;

    let 読み直し = purchase::Entity::find_by_id(p.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(読み直し.amount, 1_234_567);
}

/// **大きな金額でも桁落ちしないこと**（設計書24.2.1）。
///
/// `REAL`（f64）だと仮数部53bitを超えたところで整数を正確に表せなくなる。
/// 10,000台規模の資産を合算する用途では、そこに触れうる。
async fn 大きな金額でも桁落ちしない(db: &DatabaseConnection) {
    // f64 が正確に表せる上限（2^53）を超える値
    let 巨額: i64 = 9_007_199_254_740_993;

    let asset = fixed_asset::ActiveModel {
        item_type: Set("Device".to_owned()),
        item_id: Set(1),
        acquisition_cost: Set(巨額),
        depreciation_method: Set("straight_line".to_owned()),
        useful_life_years: Set(5),
        acquisition_date: Set(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 読み直し = fixed_asset::Entity::find_by_id(asset.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        読み直し.acquisition_cost, 巨額,
        "金額が丸められています（REALで保存されている疑い）"
    );
}

// ---------------------------------------------------------------------------
// 保存しないもの
// ---------------------------------------------------------------------------

/// **発注番号は重複してよく、注文単位の合計は番号で寄せて出ること**（10.2）。
///
/// 発注を表すテーブルは持たない。1つの注文で3台買えば、同じ番号の行が3本並ぶ。
/// **一意の制約を張っていないこと**も、ここで確かめる——張ると2台目が入らない。
async fn 発注番号は重複してよい(db: &DatabaseConnection) {
    for (item_id, amount) in [(1, 100_000), (2, 100_000), (3, 50_000)] {
        購入(db, item_id, Some("PO-0002"), amount).await;
    }
    購入(db, 4, Some("PO-9999"), 7_000).await;

    let 注文の合計: i64 = purchase::Entity::find()
        .filter(purchase::Column::OrderNumber.eq("PO-0002"))
        .all(db)
        .await
        .unwrap()
        .iter()
        .map(|p| p.amount)
        .sum();
    assert_eq!(注文の合計, 250_000);
}

/// **固定資産は廃棄日と簿価を持たないこと**（旧B-1、不変条件2）。
///
/// 廃棄は `DEVICE_ASSIGNMENT` の `Disposed` 行から、簿価は取得価額と
/// 経過期間から計算する。持つと二重管理になり、必ずずれる。
async fn 固定資産は廃棄日と簿価を持たない(db: &DatabaseConnection) {
    let asset = fixed_asset::ActiveModel {
        item_type: Set("Device".to_owned()),
        item_id: Set(1),
        acquisition_cost: Set(1_000_000),
        depreciation_method: Set("straight_line".to_owned()),
        useful_life_years: Set(5),
        acquisition_date: Set(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let json = serde_json::to_value(&asset).unwrap();
    for 持たない列 in ["disposal_date", "book_value", "current_value"] {
        assert!(
            json.get(持たない列).is_none(),
            "FIXED_ASSET が {持たない列} を持っています"
        );
    }
}

// ---------------------------------------------------------------------------
// マイルストーン（設計書10.4）
// ---------------------------------------------------------------------------

/// **予定と実績を別の列で持つこと**（設計書10.4、5.1）。
///
/// 片方に上書きすると「当初いつの予定だったか」が失われ、QCDの「D」を
/// 定量的に見るという目的が達成できなくなる。
async fn マイルストーンは予定と実績を別に持つ(db: &DatabaseConnection) {
    let p = プロジェクト(db, "納期検証").await;

    let m = milestone::ActiveModel {
        project_id: Set(p.id),
        milestone_type: Set("ServiceStart".to_owned()),
        planned_date: Set(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
        actual_date: Set(None),
        status: Set("planned".to_owned()),
        description: Set(String::new()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // 実際には2週間遅れた
    let mut active: milestone::ActiveModel = m.clone().into();
    active.actual_date = Set(Some(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap()));
    active.status = Set("completed".to_owned());
    active.update(db).await.unwrap();

    let 後 = milestone::Entity::find_by_id(m.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();

    // **当初の予定が残っていること。**これが遅延日数の算出根拠になる
    assert_eq!(
        後.planned_date,
        NaiveDate::from_ymd_opt(2026, 4, 1).unwrap(),
        "当初の予定が上書きされています"
    );
    assert_eq!(
        後.actual_date,
        Some(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap())
    );

    let 遅延 = (後.actual_date.unwrap() - 後.planned_date).num_days();
    assert_eq!(遅延, 14);
}

/// 日付が `date` のまま往復すること（設計書C-5）。
///
/// 他の履歴テーブルを `datetime` 化した理由（同日内の順序）が当てはまらない。
async fn マイルストーンの日付は日付のまま(db: &DatabaseConnection) {
    let p = プロジェクト(db, "日付検証").await;
    let 予定 = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();

    let m = milestone::ActiveModel {
        project_id: Set(p.id),
        milestone_type: Set("ServiceEnd".to_owned()),
        planned_date: Set(予定),
        actual_date: Set(None),
        status: Set("planned".to_owned()),
        description: Set(String::new()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 読み直し = milestone::Entity::find_by_id(m.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    // タイムゾーン変換で前日・翌日にずれないこと
    assert_eq!(読み直し.planned_date, 予定);
}

// ---------------------------------------------------------------------------
// 保守契約・定期費用
// ---------------------------------------------------------------------------

/// 保守期限で絞り込めること（設計書10章、旧B-2）。
///
/// 期限間近の契約はプロジェクトダッシュボードで引く。`end_date` のみが正。
async fn 保守期限で絞り込める(db: &DatabaseConnection) {
    let user = 利用者(db, "contract@example.com").await;
    let v = ベンダー(db, "保守業者", user.id).await;

    for (number, end) in [
        ("MC-2026", NaiveDate::from_ymd_opt(2026, 3, 31).unwrap()),
        ("MC-2030", NaiveDate::from_ymd_opt(2030, 3, 31).unwrap()),
    ] {
        maintenance_contract::ActiveModel {
            contract_number: Set(number.to_owned()),
            vendor_id: Set(v.id),
            start_date: Set(NaiveDate::from_ymd_opt(2025, 4, 1).unwrap()),
            end_date: Set(end),
            amount: Set(500_000),
            quote_contact: Set(String::new()),
            failure_contact: Set(String::new()),
            order_number: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    let 期限間近 = maintenance_contract::Entity::find()
        .filter(
            maintenance_contract::Column::EndDate.lt(NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()),
        )
        .all(db)
        .await
        .unwrap();

    assert_eq!(期限間近.len(), 1);
    assert_eq!(期限間近[0].contract_number, "MC-2026");
}

/// 定期費用が継続中（`end_date` が null）を表せること。
async fn 定期費用は継続中を表せる(db: &DatabaseConnection) {
    let user = 利用者(db, "recurring@example.com").await;

    let cost = recurring_cost::ActiveModel {
        item_type: Set("Project".to_owned()),
        item_id: Set(1),
        cost_type: Set("回線".to_owned()),
        vendor_id: Set(None),
        amount: Set(30_000),
        billing_cycle: Set("Monthly".to_owned()),
        start_date: Set(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
        end_date: Set(None),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    assert!(cost.end_date.is_none());
    assert_eq!(cost.amount, 30_000);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証".to_owned()),
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(false),
        locale: Set("ja".to_owned()),
        last_login_at: Set(None),
        disabled_at: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn ベンダー(db: &DatabaseConnection, name: &str, user_id: i32) -> vendor::Model {
    vendor::ActiveModel {
        name: Set(name.to_owned()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn プロジェクト(db: &DatabaseConnection, name: &str) -> project::Model {
    project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(None),
        name: Set(name.to_owned()),
        description: Set(String::new()),
        currency: Set("JPY".to_owned()),
        archived_at: Set(None),
        closure_reason: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 購入(
    db: &DatabaseConnection,
    item_id: i32,
    order_number: Option<&str>,
    amount: i64,
) -> purchase::Model {
    purchase::ActiveModel {
        item_type: Set("Device".to_owned()),
        item_id: Set(item_id),
        order_number: Set(order_number.map(str::to_owned)),
        acquired_on: Set(NaiveDate::from_ymd_opt(2026, 4, 1)),
        amount: Set(amount),
        supplier: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// `MILESTONE_DEVICE` と `DEVICE` の結び付きが張れることの確認に使う。
#[allow(dead_code)]
async fn 機器(db: &DatabaseConnection, hostname: &str) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        external_id: Set(None),
        merged_into_device_id: Set(None),
        merged_at: Set(None),
        configuration_id: Set(None),
        device_type: Set("Physical".to_owned()),
        device_category: Set(Some("Server".to_owned())),
        hostname: Set(hostname.to_owned()),
        serial_number: Set(None),
        asset_number: Set(None),
        power_watt: Set(0),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 金額は整数のまま往復する);
        全検証!(@one $用意, $属性, 大きな金額でも桁落ちしない);
        全検証!(@one $用意, $属性, 発注番号は重複してよい);
        全検証!(@one $用意, $属性, 固定資産は廃棄日と簿価を持たない);
        全検証!(@one $用意, $属性, マイルストーンは予定と実績を別に持つ);
        全検証!(@one $用意, $属性, マイルストーンの日付は日付のまま);
        全検証!(@one $用意, $属性, 保守期限で絞り込める);
        全検証!(@one $用意, $属性, 定期費用は継続中を表せる);
    };
    (@one $用意:path, $属性:meta, $名前:ident) => {
        #[tokio::test]
        #[$属性]
        async fn $名前() {
            let db = $用意().await;
            super::$名前(&db.conn).await;
        }
    };
}

mod sqlite {
    全検証!(crate::support::sqlite, cfg(all()));
}

mod postgres {
    全検証!(
        crate::support::postgres,
        ignore = "Dockerが必要。cargo test -- --ignored で実行する"
    );
}

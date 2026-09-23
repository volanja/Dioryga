//! コスト・契約管理の結合テスト（設計書10.2、10.3、24.2.2）。
//!
//! **金額が合わないことは、この画面では致命的である。**
//!
//! 24.2.2 が「按分は最小通貨単位で切り捨て、端数はすべて最終期間に加算する」と
//! 定めたのは、規則がないと**「12ヶ月分の合計が契約額と一致しない」**という
//! 状態が生じるためである。按分そのものの検証は `cost` モジュールの単体テストに
//! あり、ここでは**画面を通しても同じ結果になること**を確かめる。
//!
//! あわせて、**計算できない行を黙って除外しないこと**（10.3）を押さえる。
//! 静かに0として合算すると、年間コストが実態より小さく出る。

mod support;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, chassis_model, configuration, device, device_assignment, fixed_asset,
    maintenance_contract, mount_container, project, project_member, purchase, recurring_cost,
    vendor,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, QueryOrder, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 保守契約（設計書10.2、10.3）
// ---------------------------------------------------------------------------

/// **年をまたぐ契約が、月割りで年ごとに分かれること**（設計書10.3）。
///
/// 年割りではなく月割りである。4月開始の12ヶ月契約なら初年度9ヶ月、翌年度3ヶ月。
async fn 契約は月割りで年をまたぐ(db: &DatabaseConnection) {
    let 場 = 舞台(db, "contract@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 契約を送る(
        状態,
        &token,
        &場,
        &[
            ("contract_number", "MC-001"),
            ("vendor_id", &場.vendor_id.to_string()),
            ("start_date", "2026-04-01"),
            ("end_date", "2027-03-31"),
            ("amount", "120000"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    // 契約を機器に紐づける（10.2の中間テーブル）
    let c = 契約一覧(db).await.remove(0);
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 送信(
        状態,
        &format!(
            "/projects/{}/costs/maintenance-contracts/items",
            場.project.id
        ),
        &token,
        &[
            ("maintenance_contract_id", &c.id.to_string()),
            ("device_id", &場.device.id.to_string()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // 2026年は9ヶ月ぶん
    let body = ダッシュボード(db, &場, 2026).await;
    assert!(body.contains("90000"), "2026年が9ヶ月ぶんになっていない");

    // 2027年は3ヶ月ぶん
    let body = ダッシュボード(db, &場, 2027).await;
    assert!(body.contains("30000"), "2027年が3ヶ月ぶんになっていない");
}

/// **期間が逆の契約は合算せず、画面に列挙すること**（設計書24.2.2、10.3）。
async fn 期間が逆の契約は列挙される(db: &DatabaseConnection) {
    let 場 = 舞台(db, "badperiod@example.com").await;

    // 画面は弾くので、直接入れて取込由来の壊れた行を再現する
    let c = maintenance_contract::ActiveModel {
        contract_number: Set("MC-BAD".to_owned()),
        vendor_id: Set(場.vendor_id),
        start_date: Set(日(2027, 1, 1)),
        end_date: Set(日(2026, 1, 1)),
        amount: Set(120_000),
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

    entity::maintenance_contract_item::ActiveModel {
        maintenance_contract_id: Set(c.id),
        item_type: Set("Device".to_owned()),
        item_id: Set(場.device.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let body = ダッシュボード(db, &場, 2026).await;
    assert!(body.contains("MC-BAD"), "壊れた行が列挙されていない");
    assert!(body.contains("期間が不正"));
}

/// 画面は期間が逆の入力を弾くこと（24.2.2）。
async fn 期間が逆の契約は登録できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "period-form@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 契約を送る(
        状態,
        &token,
        &場,
        &[
            ("contract_number", "MC-X"),
            ("vendor_id", &場.vendor_id.to_string()),
            ("start_date", "2027-04-01"),
            ("end_date", "2026-03-31"),
            ("amount", "1000"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("終了日が開始日より前"));
    assert!(契約一覧(db).await.is_empty());
}

// ---------------------------------------------------------------------------
// 固定資産（設計書10.2、10.3）
// ---------------------------------------------------------------------------

/// **定額法は耐用年数ぶんだけ計上し、合計が取得価格と一致すること**（10.3）。
async fn 定額法は耐用年数ぶん計上する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "asset@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 資産を送る(
        状態,
        &token,
        &場,
        &[
            ("device_id", &場.device.id.to_string()),
            ("acquisition_cost", "500000"),
            ("depreciation_method", "straight_line"),
            ("useful_life_years", "5"),
            ("acquisition_date", "2026-04-01"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let body = ダッシュボード(db, &場, 2026).await;
    assert!(body.contains("100000"), "1年ぶんの償却費が出ていない");

    // **償却が終わったら計上しない**
    let body = ダッシュボード(db, &場, 2031).await;
    assert!(!body.contains("100000"), "償却後も計上され続けている");
}

/// **定率法は選べるが、合算に含めず理由を出すこと**（設計書10.3、Q-12）。
///
/// 黙って0にすると年間コストが実態より小さく出る。選択肢から外さないのは、
/// **定率法の資産が実在し、記録できなくすると事実を書けなくなる**ため。
async fn 定率法は選べるが合算しない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "declining@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 資産を送る(
        状態,
        &token,
        &場,
        &[
            ("device_id", &場.device.id.to_string()),
            ("acquisition_cost", "500000"),
            ("depreciation_method", "declining_balance"),
            ("useful_life_years", "5"),
            ("acquisition_date", "2026-04-01"),
        ],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "定率法が登録できない: {body}"
    );
    assert_eq!(資産一覧(db).await.len(), 1, "資産として記録されていない");

    let body = ダッシュボード(db, &場, 2026).await;
    assert!(body.contains("償却率テーブルが未実装"), "理由が出ていない");
    assert!(!body.contains("100000"), "定率法が合算されている");
}

/// **耐用年数が0以下は登録できないこと**（設計書24.2.2）。
async fn 耐用年数が零以下は登録できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "life@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 資産を送る(
        状態,
        &token,
        &場,
        &[
            ("device_id", &場.device.id.to_string()),
            ("acquisition_cost", "500000"),
            ("depreciation_method", "straight_line"),
            ("useful_life_years", "0"),
            ("acquisition_date", "2026-04-01"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("1以上"));
    assert!(資産一覧(db).await.is_empty());
}

// ---------------------------------------------------------------------------
// 定期費用（設計書10.2、10.3）
// ---------------------------------------------------------------------------

/// **`end_date` が空なら継続中として扱うこと**（設計書10.2）。
async fn 終了日が空なら継続中(db: &DatabaseConnection) {
    let 場 = 舞台(db, "recurring@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 定期費用を送る(
        状態,
        &token,
        &場,
        &[
            ("item", "Project"),
            ("cost_type", "RackRental"),
            ("amount", "12000"),
            ("billing_cycle", "Monthly"),
            ("start_date", "2026-01-01"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let r = 定期費用一覧(db).await.remove(0);
    assert!(r.end_date.is_none());

    // 12ヶ月ぶん全額が計上される
    let body = ダッシュボード(db, &場, 2026).await;
    assert!(body.contains("12000"));
}

/// **什器に付く定期費用も扱えること**（設計書10.2の多態的参照）。
async fn 什器に付く定期費用を登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "container-cost@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 定期費用を送る(
        状態,
        &token,
        &場,
        &[
            ("item", &format!("MountContainer:{}", 場.container_id)),
            ("cost_type", "RackRental"),
            ("amount", "60000"),
            ("billing_cycle", "Annual"),
            ("start_date", "2026-01-01"),
            ("end_date", "2026-12-31"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let r = 定期費用一覧(db).await.remove(0);
    assert_eq!(r.item_type, "MountContainer");
    assert_eq!(r.item_id, 場.container_id);

    // 什器の名前が画面に出る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/costs/recurring", 場.project.id),
        &token,
    )
    .await;
    assert!(body.contains("Rack-01"));
}

/// **他プロジェクトの什器は指定できないこと**（多態的参照はDB制約で守れない）。
async fn 他プロジェクトの什器は指定できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "cost-own@example.com").await;
    let 他人 = 舞台(db, "cost-other@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 定期費用を送る(
        状態,
        &token,
        &場,
        &[
            ("item", &format!("MountContainer:{}", 他人.container_id)),
            ("cost_type", "RackRental"),
            ("amount", "1000"),
            ("billing_cycle", "Monthly"),
            ("start_date", "2026-01-01"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("対象の指定が不正"));
    assert!(定期費用一覧(db).await.is_empty());
}

// ---------------------------------------------------------------------------
// 購入（設計書10.2、10.3）
// ---------------------------------------------------------------------------

/// **購入の記録は機器詳細から入れ、そこに出ること**（設計書10.2）。
///
/// `FIXED_ASSET` を持たない品目は、取得した年に全額を即時費用として計上する。
async fn 購入は機器詳細から記録する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "order@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 購入を送る(
        状態,
        &token,
        &場,
        &[
            ("order_number", "PO-001"),
            ("acquisition_date", "2026-04-01"),
            ("acquisition_cost", "100000"),
            ("supplier", "〇〇商事"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", 場.project.id, 場.device.id),
        &token,
    )
    .await;
    assert!(body.contains("PO-001"));
    assert!(body.contains("100000"));
    assert!(body.contains("〇〇商事"), "購入元が出ていない");

    // **資産計上していないので即時費用として合算される**（10.3）
    let body = ダッシュボード(db, &場, 2026).await;
    assert!(body.contains("100000"));
}

/// **購入の記録は機器1台につき1行で、保存し直すと上書きすること**（設計書10.2）。
async fn 購入の記録は上書きする(db: &DatabaseConnection) {
    let 場 = 舞台(db, "order-item@example.com").await;

    for amount in ["1000", "2000"] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, _) = 購入を送る(
            状態,
            &token,
            &場,
            &[("order_number", "PO-002"), ("acquisition_cost", amount)],
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
    }

    let 行 = 購入一覧(db).await;
    assert_eq!(行.len(), 1, "購入の記録が2行になっている");
    assert_eq!(行[0].amount, 2000);
}

/// **資産計上した機器は即時費用に含めないこと**（設計書10.3）。
///
/// 含めると減価償却費と二重に数えることになる。
async fn 資産計上した機器は即時費用にしない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "double@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    購入を送る(
        状態,
        &token,
        &場,
        &[
            ("order_number", "PO-003"),
            ("acquisition_date", "2026-04-01"),
            ("acquisition_cost", "500000"),
        ],
    )
    .await;

    let (状態, token) = 認証済み(db, &場.user).await;
    資産を送る(
        状態,
        &token,
        &場,
        &[
            ("device_id", &場.device.id.to_string()),
            ("acquisition_cost", "500000"),
            ("depreciation_method", "straight_line"),
            ("useful_life_years", "5"),
            ("acquisition_date", "2026-04-01"),
        ],
    )
    .await;

    let body = ダッシュボード(db, &場, 2026).await;
    // 償却費 100,000 だけが計上され、即時費用 500,000 は入らない
    assert!(body.contains("100000"));
    assert!(!body.contains("600000"), "二重に数えている");
}

/// **取得日の無い購入は合算せず列挙すること**（設計書10.3）。
///
/// どの年に計上するか決められない。黙って落とすと、金額が小さいのが実態なのか
/// 入力漏れなのかを利用者が判別できない。
async fn 取得日の無い購入は列挙される(db: &DatabaseConnection) {
    let 場 = 舞台(db, "currency@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    購入を送る(
        状態,
        &token,
        &場,
        &[
            ("order_number", "PO-NODATE"),
            ("acquisition_cost", "100000"),
        ],
    )
    .await;

    let body = ダッシュボード(db, &場, 2026).await;
    assert!(
        body.contains(&場.device.hostname),
        "取得日の無い購入が列挙されていない"
    );
    assert!(body.contains("取得日が無い"));
}

// ---------------------------------------------------------------------------
// 金額の桁（設計書24.2.1）
// ---------------------------------------------------------------------------

/// **円に小数は無いこと。**桁数は通貨から決まる（24.2.1）。
async fn 円に小数は入れられない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "digits@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 資産を送る(
        状態,
        &token,
        &場,
        &[
            ("device_id", &場.device.id.to_string()),
            ("acquisition_cost", "1234.5"),
            ("depreciation_method", "straight_line"),
            ("useful_life_years", "5"),
            ("acquisition_date", "2026-04-01"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("小数点以下の桁数"));
    assert!(資産一覧(db).await.is_empty());
}

// ---------------------------------------------------------------------------
// 権限
// ---------------------------------------------------------------------------

/// **Viewerは編集できないこと。**閲覧はできる（3章）。
async fn 閲覧者はコストを編集できない(db: &DatabaseConnection) {
    let 場 = 舞台の役つき(db, "cost-viewer@example.com", "Viewer").await;

    // **フォームとして妥当な内容を送る。**欠けているとaxumの抽出が先に落ち、
    // 認可まで届かない（422になり、403かどうかを確かめられない）
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 契約を送る(
        状態,
        &token,
        &場,
        &[
            ("contract_number", "MC-V"),
            ("vendor_id", &場.vendor_id.to_string()),
            ("start_date", "2026-04-01"),
            ("end_date", "2027-03-31"),
            ("amount", "1000"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(契約一覧(db).await.is_empty());

    for path in ["", "/maintenance-contracts", "/fixed-assets", "/recurring"] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, _) = 取得(
            状態,
            &format!("/projects/{}/costs{path}", 場.project.id),
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
    device: device::Model,
    vendor_id: i32,
    container_id: i32,
}

fn 日(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

async fn 舞台(db: &DatabaseConnection, email: &str) -> 舞台情報 {
    舞台の役つき(db, email, "Operator").await
}

async fn 舞台の役つき(db: &DatabaseConnection, email: &str, role: &str) -> 舞台情報 {
    let user = 利用者(db, email).await;
    let p = プロジェクト(db, email).await;
    メンバー(db, user.id, p.id, role).await;

    let v = vendor::ActiveModel {
        name: Set(format!("ベンダー-{email}")),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let m = chassis_model::ActiveModel {
        vendor_id: Set(v.id),
        model_name: Set(format!("型-{email}")),
        device_category: Set("Server".to_owned()),
        height_u: Set(1),
        mount_form: Set("RackU".to_owned()),
        rack_width: Set(Some("Full".to_owned())),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let c = configuration::ActiveModel {
        chassis_model_id: Set(m.id),
        name: Set("標準構成".to_owned()),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(Some(c.id)),
        hostname: Set(format!("srv-{}", p.id)),
        device_type: Set("Physical".to_owned()),
        power_watt: Set(350),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(p.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let container = mount_container::ActiveModel {
        name: Set("Rack-01".to_owned()),
        container_type: Set("Rack".to_owned()),
        location_type: Set("Project".to_owned()),
        location_id: Set(p.id),
        capacity: Set(Some(42)),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    舞台情報 {
        user,
        project: p,
        device: d,
        vendor_id: v.id,
        container_id: container.id,
    }
}

async fn ダッシュボード(db: &DatabaseConnection, 場: &舞台情報, year: i32) -> String {
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(
        状態,
        &format!("/projects/{}/costs?year={year}", 場.project.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body
}

async fn 契約を送る(
    state: AppState,
    token: &str,
    場: &舞台情報,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/projects/{}/costs/maintenance-contracts", 場.project.id),
        token,
        fields,
    )
    .await
}

async fn 資産を送る(
    state: AppState,
    token: &str,
    場: &舞台情報,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/projects/{}/costs/fixed-assets", 場.project.id),
        token,
        fields,
    )
    .await
}

async fn 定期費用を送る(
    state: AppState,
    token: &str,
    場: &舞台情報,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/projects/{}/costs/recurring", 場.project.id),
        token,
        fields,
    )
    .await
}

async fn 購入を送る(
    state: AppState,
    token: &str,
    場: &舞台情報,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(
        state,
        &format!(
            "/projects/{}/devices/{}/purchase",
            場.project.id, 場.device.id
        ),
        token,
        fields,
    )
    .await
}

async fn 契約一覧(db: &DatabaseConnection) -> Vec<maintenance_contract::Model> {
    maintenance_contract::Entity::find()
        .order_by_asc(maintenance_contract::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 資産一覧(db: &DatabaseConnection) -> Vec<fixed_asset::Model> {
    fixed_asset::Entity::find().all(db).await.unwrap()
}

async fn 定期費用一覧(db: &DatabaseConnection) -> Vec<recurring_cost::Model> {
    recurring_cost::Entity::find()
        .order_by_asc(recurring_cost::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 購入一覧(db: &DatabaseConnection) -> Vec<purchase::Model> {
    purchase::Entity::find()
        .order_by_asc(purchase::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(email.to_owned()),
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

async fn メンバー(db: &DatabaseConnection, user_id: i32, project_id: i32, role: &str) {
    project_member::ActiveModel {
        project_id: Set(project_id),
        user_id: Set(user_id),
        role: Set(role.to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 認証済み(db: &DatabaseConnection, user: &app_user::Model) -> (AppState, String) {
    let config = 設定();
    let (setup, _) = SetupState::initialize(db).await.unwrap();
    let state = AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
        staged: Default::default(),
    };
    let (_, token) = session::create(
        db,
        user.id,
        "127.0.0.1",
        "test",
        &state.config.session,
        Utc::now(),
    )
    .await
    .unwrap();
    (state, token.as_str().to_owned())
}

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

fn cookie_header(token: &str) -> String {
    format!("{}={token}", session::COOKIE_NAME)
}

async fn 取得(state: AppState, uri: &str, token: &str) -> (StatusCode, String) {
    let res = router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie_header(token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    分解(res).await
}

async fn 送信(
    state: AppState,
    uri: &str,
    token: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    let csrf = dioryga::auth::csrf::derive(token);
    let mut pairs: Vec<(String, String)> = vec![(dioryga::auth::csrf::FIELD_NAME.to_owned(), csrf)];
    for (key, value) in fields {
        pairs.push(((*key).to_owned(), (*value).to_owned()));
    }
    let body = serde_urlencoded::to_string(&pairs).unwrap();

    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie_header(token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    分解(res).await
}

async fn 分解(res: Response<Body>) -> (StatusCode, String) {
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 契約は月割りで年をまたぐ);
        全検証!(@one $用意, $属性, 期間が逆の契約は列挙される);
        全検証!(@one $用意, $属性, 期間が逆の契約は登録できない);
        全検証!(@one $用意, $属性, 定額法は耐用年数ぶん計上する);
        全検証!(@one $用意, $属性, 定率法は選べるが合算しない);
        全検証!(@one $用意, $属性, 耐用年数が零以下は登録できない);
        全検証!(@one $用意, $属性, 終了日が空なら継続中);
        全検証!(@one $用意, $属性, 什器に付く定期費用を登録できる);
        全検証!(@one $用意, $属性, 他プロジェクトの什器は指定できない);
        全検証!(@one $用意, $属性, 購入は機器詳細から記録する);
        全検証!(@one $用意, $属性, 購入の記録は上書きする);
        全検証!(@one $用意, $属性, 資産計上した機器は即時費用にしない);
        全検証!(@one $用意, $属性, 取得日の無い購入は列挙される);
        全検証!(@one $用意, $属性, 円に小数は入れられない);
        全検証!(@one $用意, $属性, 閲覧者はコストを編集できない);
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

//! プロジェクトダッシュボードの結合テスト（設計書16.1のB領域、16.3）。
//!
//! # 何を確かめているか
//!
//! **台数は「現在このプロジェクトにあるもの」だけを数えること。**コスト画面が
//! 使っている「過去に所属したものも含む」判定（A-6）をそのまま持ち込むと、
//! **移管した機器を今の台数に数える。**
//!
//! **期限を過ぎたものが枠から消えないこと。**切れた契約・遅れたマイルストーンが
//! 最初に気付くべきものであり、窓を「今日から90日後まで」と読むと過ぎた瞬間に
//! 画面から消える。
//!
//! **未完了のチケットを期限で絞らないこと。**期限が未設定のものが最も
//! 見落とされやすい。

mod support;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::{Duration, Utc};
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, device, device_assignment, maintenance_contract, maintenance_contract_item,
    milestone, project, project_member, vendor, work_order,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 機器数
// ---------------------------------------------------------------------------

/// **移管した機器を今の台数に数えないこと。**
///
/// `DEVICE_ASSIGNMENT` は履歴であり、移管しても行は残る（`to_date` が入るだけ）。
/// A-6の判定をそのまま使うと、出ていった機器が台数に居座る。
async fn 移管した機器は数えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-moved@example.com", "Operator").await;

    // 出ていった機器。行は残るが `to_date` が入っている
    let 出た = 機器(db, "srv-departed", "running").await;
    device_assignment::ActiveModel {
        device_id: Set(出た.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(場.project.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(30)),
        to_date: Set(Some(Utc::now() - Duration::days(1))),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    // 舞台が用意する稼働中1台だけ。出ていった1台を足して2にしていないこと
    assert_eq!(
        抜き出す(&body, "稼働中"),
        "1",
        "移管した機器を数えています: {body}"
    );
}

/// **予約中は稼働中と分けて数えること**（設計書11.6、12.5）。
async fn 予約中は分けて数える(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-plan@example.com", "Operator").await;

    let 予約 = 機器(db, "srv-planned", "planned").await;
    割り当て(db, 予約.id, 場.project.id).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    // 稼働中1台・予約中1台。合算して2台と出していないこと
    assert_eq!(抜き出す(&body, "稼働中"), "1", "{body}");
    assert_eq!(抜き出す(&body, "予約中"), "1", "{body}");
}

/// **5つの状態すべてに枠を出すこと**（#170）。
///
/// 0件の状態を落とすと、並びが詰まって位置で読めなくなる。
async fn 状態は5つとも枠を出す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-metrics@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    for (ラベル, 台数) in [
        ("稼働中", "1"),
        ("予約中", "0"),
        ("構築中", "0"),
        ("修理中", "0"),
        ("故障", "0"),
    ] {
        assert_eq!(抜き出す(&body, ラベル), 台数, "{ラベル}: {body}");
    }
}

/// **故障・修理中の機器を、起票への導線とともに出すこと**（#170）。
///
/// 気付いても一覧を開き直して機器を探すところから始めるのでは、作業につながらない。
async fn 対応が必要な機器を出す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-attention@example.com", "Operator").await;

    let 故障 = 機器(db, "srv-failed", "failed").await;
    割り当て(db, 故障.id, 場.project.id).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert!(body.contains("対応が必要な機器"), "{body}");
    assert!(body.contains("srv-failed"), "{body}");
    // 未起票なので、起票への導線が出る
    assert!(
        body.contains(&format!(
            r#"href="/projects/{}/work-orders/new""#,
            場.project.id
        )),
        "{body}"
    );
}

/// **故障も修理中も無ければ、表ごと出さないこと**（#170）。
async fn 対応が必要な機器が無ければ出さない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-no-attention@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert!(!body.contains("対応が必要な機器"), "{body}");
}

// ---------------------------------------------------------------------------
// 期限
// ---------------------------------------------------------------------------

/// **期限を過ぎたマイルストーンが消えないこと。**
///
/// 窓を「今日から90日後まで」と読むと、**過ぎた瞬間に画面から消える。**
/// 遅れているものこそ最初に気付くべきである。
async fn 期限を過ぎたものも出す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-overdue@example.com", "Operator").await;

    milestone::ActiveModel {
        project_id: Set(場.project.id),
        milestone_type: Set("ServiceStart".to_owned()),
        planned_date: Set((Utc::now() - Duration::days(10)).date_naive()),
        actual_date: Set(None),
        status: Set("planned".to_owned()),
        description: Set("遅れている開始".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("遅れている開始"), "{body}");
    assert!(body.contains("期限超過"), "超過の印がありません: {body}");
}

/// **窓の外のマイルストーンは出さないこと。**出すと一覧と変わらない。
async fn 遠い予定は出さない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-far@example.com", "Operator").await;

    milestone::ActiveModel {
        project_id: Set(場.project.id),
        milestone_type: Set("ServiceEnd".to_owned()),
        planned_date: Set((Utc::now() + Duration::days(200)).date_naive()),
        actual_date: Set(None),
        status: Set("planned".to_owned()),
        description: Set("ずっと先の終了".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert!(!body.contains("ずっと先の終了"), "{body}");
}

/// **完了したマイルストーンは出さないこと。**完了したものに期限は無い。
async fn 完了したものは出さない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-done@example.com", "Operator").await;

    milestone::ActiveModel {
        project_id: Set(場.project.id),
        milestone_type: Set("ServiceStart".to_owned()),
        planned_date: Set((Utc::now() - Duration::days(5)).date_naive()),
        actual_date: Set(Some((Utc::now() - Duration::days(5)).date_naive())),
        status: Set("completed".to_owned()),
        description: Set("済んだ開始".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert!(!body.contains("済んだ開始"), "{body}");
}

/// **期限が未設定のチケットも出すこと。**
///
/// チケットだけは期限で絞らない。16.1が求めているのは「未完了のサマリ」であり、
/// **期限が未設定のものが最も見落とされやすい。**窓を掛けると消える。
async fn 期限のないチケットも出す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-nodue@example.com", "Operator").await;

    work_order::ActiveModel {
        project_id: Set(場.project.id),
        target_project_id: Set(None),
        device_id: Set(None),
        part_instance_id: Set(None),
        work_type: Set("Repair".to_owned()),
        title: Set("期限のない修理".to_owned()),
        description: Set(String::new()),
        primary_assignee_id: Set(None),
        secondary_assignee_id: Set(None),
        due_date: Set(None),
        status: Set("planned".to_owned()),
        planned_at: Set(None),
        executed_at: Set(None),
        completed_at: Set(None),
        cancelled_at: Set(None),
        cancelled_reason: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert!(body.contains("期限のない修理"), "{body}");
    assert!(body.contains("期限なし"), "{body}");
}

/// **完了・中止したチケットは出さないこと。**
async fn 完了したチケットは出さない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-wo-done@example.com", "Operator").await;

    for (title, status) in [("済んだ修理", "completed"), ("やめた修理", "cancelled")] {
        work_order::ActiveModel {
            project_id: Set(場.project.id),
            target_project_id: Set(None),
            device_id: Set(None),
            part_instance_id: Set(None),
            work_type: Set("Repair".to_owned()),
            title: Set(title.to_owned()),
            description: Set(String::new()),
            primary_assignee_id: Set(None),
            secondary_assignee_id: Set(None),
            due_date: Set(None),
            status: Set(status.to_owned()),
            planned_at: Set(None),
            executed_at: Set(None),
            completed_at: Set(None),
            cancelled_at: Set(None),
            cancelled_reason: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert!(!body.contains("済んだ修理"), "{body}");
    assert!(!body.contains("やめた修理"), "{body}");
}

/// **満了が近い契約が出ること**（設計書10.2）。
///
/// **契約はプロジェクトを直接持たない。**`MAINTENANCE_CONTRACT_ITEM` から機器を
/// 経由して辿る。判定は `server::cost` と共有しており、**コスト画面に出る契約と
/// ここに出る契約が食い違ってはならない。**
async fn 満了間近の契約が出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-contract@example.com", "Operator").await;
    let d = 場の機器(db, &場).await;

    契約(db, 場.user.id, "CT-SOON", d.id, 30).await;
    契約(db, 場.user.id, "CT-LATER", d.id, 400).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("CT-SOON"), "{body}");
    // 窓の外の契約まで出すと、コスト画面の一覧と変わらない
    assert!(!body.contains("CT-LATER"), "{body}");
}

/// **既に切れた契約も出ること。**過ぎた瞬間に消えては、更新漏れに気付けない。
async fn 切れた契約も出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-expired@example.com", "Operator").await;
    let d = 場の機器(db, &場).await;

    契約(db, 場.user.id, "CT-EXPIRED", d.id, -20).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;

    assert!(body.contains("CT-EXPIRED"), "{body}");
    assert!(body.contains("期限超過"), "{body}");
}

// ---------------------------------------------------------------------------
// 認可
// ---------------------------------------------------------------------------

/// **閲覧者も開けること。**ダッシュボードは読むだけの画面である。
async fn 閲覧者も開ける(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-viewer@example.com", "Viewer").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// **メンバーでない者は開けないこと**（設計書3章）。
async fn 他人のプロジェクトは開けない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "dash-owner@example.com", "Operator").await;
    let よそ者 = 利用者(db, "dash-stranger@example.com").await;

    let (状態, token) = 認証済み(db, &よそ者).await;
    let (status, _) = 取得(状態, &format!("/projects/{}", 場.project.id), &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 用意
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
}

/// 稼働中の機器を1台だけ持つプロジェクト。
async fn 舞台(db: &DatabaseConnection, email: &str, role: &str) -> 舞台情報 {
    let user = 利用者(db, email).await;
    let p = プロジェクト(db, email).await;
    メンバー(db, user.id, p.id, role).await;

    let d = 機器(db, &format!("srv-{}", p.id), "running").await;
    割り当て(db, d.id, p.id).await;

    舞台情報 { user, project: p }
}

/// このプロジェクトにある機器を1つ返す。契約の紐づけ先に要る。
async fn 場の機器(db: &DatabaseConnection, 場: &舞台情報) -> device::Model {
    device::Entity::find()
        .filter(device::Column::Hostname.eq(format!("srv-{}", 場.project.id)))
        .one(db)
        .await
        .unwrap()
        .expect("舞台の機器がありません")
}

/// 満了まで `days` 日の保守契約を作り、機器に紐づける（10.2）。
async fn 契約(db: &DatabaseConnection, user_id: i32, number: &str, device_id: i32, days: i64) {
    let v = vendor::ActiveModel {
        name: Set(format!("ベンダー-{number}")),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let c = maintenance_contract::ActiveModel {
        contract_number: Set(number.to_owned()),
        vendor_id: Set(v.id),
        start_date: Set((Utc::now() - Duration::days(365)).date_naive()),
        end_date: Set((Utc::now() + Duration::days(days)).date_naive()),
        amount: Set(120_000),
        quote_contact: Set(String::new()),
        failure_contact: Set(String::new()),
        purchase_order_id: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    maintenance_contract_item::ActiveModel {
        maintenance_contract_id: Set(c.id),
        item_type: Set("Device".to_owned()),
        item_id: Set(device_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 機器(db: &DatabaseConnection, hostname: &str, status: &str) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(None),
        hostname: Set(hostname.to_owned()),
        device_type: Set("Physical".to_owned()),
        power_watt: Set(350),
        status: Set(status.to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 割り当て(db: &DatabaseConnection, device_id: i32, project_id: i32) {
    device_assignment::ActiveModel {
        device_id: Set(device_id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(project_id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
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

async fn 取得(state: AppState, uri: &str, token: &str) -> (StatusCode, String) {
    let res = router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, format!("{}={token}", session::COOKIE_NAME))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// 見出しの直後に出る数字を取り出す。
///
/// **台数を数字として確かめるために要る。**見出しの有無だけでは、
/// 稼働中と予約中を取り違えても気付けない。
fn 抜き出す(body: &str, 見出し: &str) -> String {
    let 後ろ = body
        .split_once(見出し)
        .unwrap_or_else(|| panic!("見出しがありません: {見出し}"))
        .1;
    let 開始 = 後ろ
        .find("<p class=\"metric\">")
        .expect("metric が続いていません")
        + "<p class=\"metric\">".len();
    let 残り = &後ろ[開始..];
    残り[..残り.find('<').expect("閉じタグがありません")]
        .trim()
        .to_owned()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 移管した機器は数えない);
        全検証!(@one $用意, $属性, 予約中は分けて数える);
        全検証!(@one $用意, $属性, 状態は5つとも枠を出す);
        全検証!(@one $用意, $属性, 対応が必要な機器を出す);
        全検証!(@one $用意, $属性, 対応が必要な機器が無ければ出さない);
        全検証!(@one $用意, $属性, 期限を過ぎたものも出す);
        全検証!(@one $用意, $属性, 遠い予定は出さない);
        全検証!(@one $用意, $属性, 完了したものは出さない);
        全検証!(@one $用意, $属性, 期限のないチケットも出す);
        全検証!(@one $用意, $属性, 完了したチケットは出さない);
        全検証!(@one $用意, $属性, 満了間近の契約が出る);
        全検証!(@one $用意, $属性, 切れた契約も出る);
        全検証!(@one $用意, $属性, 閲覧者も開ける);
        全検証!(@one $用意, $属性, 他人のプロジェクトは開けない);
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

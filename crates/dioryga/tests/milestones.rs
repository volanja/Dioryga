//! マイルストーンと電力集計の結合テスト（設計書10.4、12.5〜12.7）。
//!
//! # マイルストーン（10.4）
//!
//! **`planned_date` を上書きしないこと**が要点である。上書きすると
//! 「当初いつの予定だったか」が失われ、QCDの「D」を定量的に見られなくなる。
//!
//! # 電力集計（12.5）
//!
//! **仮想マシンとコンテナを数えないこと。**電力を引いているのはホストであり、
//! 13章でVMを `DEVICE` として扱えるようにした結果、素直に合算すると**二重に
//! 数える。**

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
    app_user, chassis_model, configuration, device, device_assignment, device_mount, milestone,
    milestone_device, mount_container, project, project_member, vendor,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, QueryOrder, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// マイルストーン（設計書10.4）
// ---------------------------------------------------------------------------

/// **完了しても `planned_date` を書き換えないこと**（設計書10.4）。
///
/// 上書きすると「当初いつの予定だったか」が失われる。**計画と実績のズレ
/// （納期遵守率）が可視化できる**ことが、この表を置いた理由である。
async fn 完了しても予定日は残る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "milestone@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/milestones", 場.project.id),
        &token,
        &[
            ("milestone_type", "ServiceStart"),
            ("planned_date", "2026-04-01"),
            ("description", "サービス開始"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let m = マイルストーン一覧(db).await.remove(0);
    assert_eq!(m.planned_date, 日(2026, 4, 1));
    assert!(m.actual_date.is_none());

    // 予定より遅れて完了
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/milestones/complete", 場.project.id),
        &token,
        &[("id", &m.id.to_string()), ("actual_date", "2026-04-15")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let m = マイルストーン一覧(db).await.remove(0);
    assert_eq!(m.planned_date, 日(2026, 4, 1), "予定日が書き換わっている");
    assert_eq!(m.actual_date, Some(日(2026, 4, 15)));
    assert_eq!(m.status, "completed");

    // **ズレが画面に出る**
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/milestones", 場.project.id),
        &token,
    )
    .await;
    assert!(body.contains("14日"), "遅れが出ていない");
}

/// **中止に実績日を付けないこと。**起きなかったことに日付を残さない。
async fn 中止には実績日を付けない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "cancel@example.com").await;
    let m = マイルストーンを作る(db, &場, "ServiceEnd", "2026-04-01").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/milestones/complete", 場.project.id),
        &token,
        &[("id", &m.id.to_string()), ("cancel", "1")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let m = マイルストーン一覧(db).await.remove(0);
    assert_eq!(m.status, "cancelled");
    assert!(m.actual_date.is_none(), "中止に実績日が付いている");
    assert_eq!(m.planned_date, 日(2026, 4, 1));
}

/// 機器を結べること（設計書10.4）。
async fn 対象機器を結べる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "ms-device@example.com").await;
    let m = マイルストーンを作る(db, &場, "ServiceUpdate", "2026-06-01").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/milestones/devices", 場.project.id),
        &token,
        &[
            ("milestone_id", &m.id.to_string()),
            ("device_id", &場.device.id.to_string()),
            ("change_type", "addition"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    assert_eq!(
        milestone_device::Entity::find()
            .all(db)
            .await
            .unwrap()
            .len(),
        1
    );

    // 同じ機器を2回は結ばない
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/milestones/devices", 場.project.id),
        &token,
        &[
            ("milestone_id", &m.id.to_string()),
            ("device_id", &場.device.id.to_string()),
            ("change_type", "removal"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既にこのマイルストーンの対象"));
}

/// **他プロジェクトのマイルストーンは操作できないこと**（3章）。
async fn 他プロジェクトのマイルストーンは操作できない(
    db: &DatabaseConnection,
) {
    let 場 = 舞台(db, "ms-owner@example.com").await;
    let 他人 = 舞台(db, "ms-outsider@example.com").await;
    let m = マイルストーンを作る(db, &場, "ServiceStart", "2026-04-01").await;

    let (状態, token) = 認証済み(db, &他人.user).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/milestones/complete", 他人.project.id),
        &token,
        &[("id", &m.id.to_string())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    assert_eq!(マイルストーン一覧(db).await[0].status, "planned");
}

// ---------------------------------------------------------------------------
// 電力集計（設計書12.5、12.6）
// ---------------------------------------------------------------------------

/// **仮想マシンとコンテナを合算しないこと**（設計書12.5、13章）。
///
/// 電力を引いているのはホストである。VMを物理サーバと同じように数えると
/// **二重に数える。**
async fn 仮想マシンは電力を数えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "power-vm@example.com").await;

    // 物理サーバ（350W）は 舞台 が作っている。搭載する
    搭載(db, &場, 場.device.id, 1).await;

    // 同じ什器にVMを載せる
    let vm = 機器(db, &場, "vm-01", "Virtual", "running", 200).await;
    搭載(db, &場, vm.id, 2).await;

    let body = 電力画面(db, &場).await;
    assert!(body.contains("350 W"), "物理サーバが数えられていない");
    assert!(!body.contains("550 W"), "仮想マシンを数えている");
    // **数えていないことを示す**
    assert!(body.contains("<td>1</td>"), "仮想の台数が出ていない");
}

/// **予約中の機器を現在値に混ぜないこと**（設計書11.6）。
///
/// まだ電力を引いていないが、引く予定である。両方を並べて出す。
async fn 予約中は分けて出す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "power-plan@example.com").await;
    搭載(db, &場, 場.device.id, 1).await;

    let 予約 = 機器(db, &場, "srv-plan", "Physical", "planned", 500).await;
    搭載(db, &場, 予約.id, 2).await;

    let body = 電力画面(db, &場).await;
    assert!(body.contains("350 W"), "稼働中が350Wでない");
    assert!(body.contains("850 W"), "予約込みが850Wでない");
}

/// 搭載していない機器は数えないこと。
async fn 搭載していない機器は数えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "power-unmounted@example.com").await;
    // 舞台の機器は搭載しない

    let body = 電力画面(db, &場).await;
    assert!(body.contains("0 W"));
    assert!(!body.contains("350 W"));
}

/// **Viewerも閲覧できること**（3章。編集の操作を持たない画面である）。
async fn 閲覧者も電力を見られる(db: &DatabaseConnection) {
    let 場 = 舞台の役つき(db, "power-viewer@example.com", "Viewer").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 取得(状態, &format!("/projects/{}/power", 場.project.id), &token).await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
    device: device::Model,
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
        container_id: container.id,
    }
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

async fn マイルストーンを作る(
    db: &DatabaseConnection,
    場: &舞台情報,
    kind: &str,
    planned: &str,
) -> milestone::Model {
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/milestones", 場.project.id),
        &token,
        &[("milestone_type", kind), ("planned_date", planned)],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    マイルストーン一覧(db).await.pop().unwrap()
}

async fn マイルストーン一覧(db: &DatabaseConnection) -> Vec<milestone::Model> {
    milestone::Entity::find()
        .order_by_asc(milestone::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 電力画面(db: &DatabaseConnection, 場: &舞台情報) -> String {
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &format!("/projects/{}/power", 場.project.id), &token).await;
    assert_eq!(status, StatusCode::OK);
    body
}

/// 什器に搭載する。
async fn 搭載(db: &DatabaseConnection, 場: &舞台情報, device_id: i32, position: i32) {
    device_mount::ActiveModel {
        device_id: Set(device_id),
        container_id: Set(Some(場.container_id)),
        position: Set(Some(position)),
        horizontal_position: Set(Some("Full".to_owned())),
        depth_position: Set(Some("Full".to_owned())),
        host_device_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

/// 追加の機器を作る。
async fn 機器(
    db: &DatabaseConnection,
    場: &舞台情報,
    hostname: &str,
    device_type: &str,
    status: &str,
    watt: i32,
) -> device::Model {
    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(None),
        hostname: Set(hostname.to_owned()),
        device_type: Set(device_type.to_owned()),
        power_watt: Set(watt),
        status: Set(status.to_owned()),
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
        location_id: Set(Some(場.project.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    d
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 完了しても予定日は残る);
        全検証!(@one $用意, $属性, 中止には実績日を付けない);
        全検証!(@one $用意, $属性, 対象機器を結べる);
        全検証!(@one $用意, $属性, 他プロジェクトのマイルストーンは操作できない);
        全検証!(@one $用意, $属性, 仮想マシンは電力を数えない);
        全検証!(@one $用意, $属性, 予約中は分けて出す);
        全検証!(@one $用意, $属性, 搭載していない機器は数えない);
        全検証!(@one $用意, $属性, 閲覧者も電力を見られる);
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

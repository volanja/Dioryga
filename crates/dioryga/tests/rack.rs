//! 什器・搭載位置・ラック図の結合テスト（設計書12章、12.9）。
//!
//! **図が出ることではなく、12.3の業務ルールが守られているか**を確かめる。
//! 重複配置・半width・0Uサイドマウントの規則はDB制約で表現できないため、
//! アプリケーション層が唯一の防波堤である。
//!
//! あわせて、**誤った状態を隠さずに描く**（不変条件6）ことも確かめる。
//! 収容能力を超えた機器を図から落とすと、誤登録に気付けなくなる。

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
    app_user, chassis_model, configuration, device, device_assignment, device_mount,
    mount_container, project, project_member, vendor,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 什器
// ---------------------------------------------------------------------------

/// 什器を登録できること。**Deskは `capacity` を持たない**（設計書12.3）。
async fn 什器を登録できる(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "container@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/containers", p.id),
        &token,
        &[
            ("name", "Rack-01"),
            ("container_type", "Rack"),
            ("capacity", "42"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &user).await;
    送信(
        状態,
        &format!("/projects/{}/containers", p.id),
        &token,
        &[("name", "作業机"), ("container_type", "Desk")],
    )
    .await;

    let 一覧 = mount_container::Entity::find()
        .order_by_asc(mount_container::Column::Id)
        .all(db)
        .await
        .unwrap();
    assert_eq!(一覧.len(), 2);
    assert_eq!(一覧[0].capacity, Some(42));
    assert_eq!(一覧[1].capacity, None, "机が格子を持ってしまっている");
}

/// 語彙外の種別を拒否すること（Q-21）。
async fn 語彙外の種別は拒否される(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "vocab@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/containers", p.id),
        &token,
        &[("name", "謎の什器"), ("container_type", "Cabinet")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("種別の値が不正"));
    assert_eq!(
        mount_container::Entity::find().all(db).await.unwrap().len(),
        0
    );
}

// ---------------------------------------------------------------------------
// 搭載の業務ルール（設計書12.3）
// ---------------------------------------------------------------------------

/// 搭載できること。**行は履歴として開く**（不変条件1）。
async fn 機器を搭載できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "mount@example.com").await;
    let d = 機器(db, &場, "srv-01", 2, "RackU", None, "running").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 搭載(状態, &token, &場, d.id, &[("position", "10")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 行 = 現行の搭載(db, d.id).await.unwrap();
    assert_eq!(行.container_id, Some(場.container.id));
    assert_eq!(行.position, Some(10));
    assert!(行.to_date.is_none());
}

/// **同じ位置に重ねられないこと**（設計書12.3）。
async fn 同じ位置には重ねられない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "occupied@example.com").await;
    let a = 機器(db, &場, "srv-a", 2, "RackU", None, "running").await;
    let b = 機器(db, &場, "srv-b", 1, "RackU", None, "running").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    搭載(状態, &token, &場, a.id, &[("position", "10")]).await;

    // a は 10U から 2U ぶん。11U は a が占めている
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 搭載(状態, &token, &場, b.id, &[("position", "11")]).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に機器があります"));
    assert!(現行の搭載(db, b.id).await.is_none());
}

/// **左右に分ければ同じUに置けること**（設計書12.3）。
async fn 半幅なら左右に置ける(db: &DatabaseConnection) {
    let 場 = 舞台(db, "half@example.com").await;
    let a = 機器(db, &場, "half-a", 1, "RackU", Some("Half"), "running").await;
    let b = 機器(db, &場, "half-b", 1, "RackU", Some("Half"), "running").await;

    for (d, h) in [(a.id, "Left"), (b.id, "Right")] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = 搭載(
            状態,
            &token,
            &場,
            d,
            &[("position", "20"), ("horizontal_position", h)],
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{h} が置けない: {body}");
    }

    assert!(現行の搭載(db, a.id).await.is_some());
    assert!(現行の搭載(db, b.id).await.is_some());
}

/// **全幅の型には左右を指定できないこと**（設計書12.3）。
async fn 全幅の型は左右に置けない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "fullwidth@example.com").await;
    let d = 機器(db, &場, "full-01", 1, "RackU", Some("Full"), "running").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 搭載(
        状態,
        &token,
        &場,
        d.id,
        &[("position", "5"), ("horizontal_position", "Left")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("半幅（Half）の型だけ"));
    assert!(現行の搭載(db, d.id).await.is_none());
}

/// **前後同居は許すが注意喚起すること**（設計書12.3）。
///
/// 排熱上は推奨しないが実在するため、拒否はしない。
async fn 前後同居は許すが警告する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "frontrear@example.com").await;
    let a = 機器(db, &場, "front-01", 1, "RackU", None, "running").await;
    let b = 機器(db, &場, "rear-01", 1, "RackU", None, "running").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    搭載(
        状態,
        &token,
        &場,
        a.id,
        &[("position", "30"), ("depth_position", "Front")],
    )
    .await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 搭載(
        状態,
        &token,
        &場,
        b.id,
        &[("position", "30"), ("depth_position", "Rear")],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "拒否されてしまっている");
    assert!(body.contains("排熱上は推奨されません"), "警告が出ていない");
    assert!(現行の搭載(db, b.id).await.is_some(), "登録されていない");
}

/// **収容能力の超過は警告に留め、登録は通すこと**（不変条件6）。
async fn 収容能力の超過は警告に留まる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "capacity@example.com").await;
    let d = 機器(db, &場, "over-01", 2, "RackU", None, "running").await;

    // ラックは 42U。41U に 2U の機器を置くと 42U を跨ぐので収まるが、42U だと溢れる
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 搭載(状態, &token, &場, d.id, &[("position", "42")]).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("収容能力を超えています"));
    assert!(現行の搭載(db, d.id).await.is_some(), "登録が拒否されている");
}

/// **範囲外の機器も図に出すこと**（不変条件6、設計書12.9）。
///
/// 隠すと誤登録に気付けない。
async fn 範囲外の機器も図に出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "outofrange@example.com").await;
    let d = 機器(db, &場, "over-02", 1, "RackU", None, "running").await;
    搭載行(db, &場, d.id, Some(99), None, None, None).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &図のあて先(&場), &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("収容能力の範囲外"));
    assert!(body.contains("over-02"), "範囲外の機器が図から消えている");
}

/// **0Uサイドマウントに位置を指定できないこと**（設計書12.3）。
async fn サイドマウントに位置は指定できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "rackside@example.com").await;
    let pdu = 機器(db, &場, "pdu-01", 1, "RackSide", None, "running").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 搭載(状態, &token, &場, pdu.id, &[("position", "1")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("位置は指定できません"));

    // 位置なしなら通る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 搭載(状態, &token, &場, pdu.id, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(現行の搭載(db, pdu.id).await.unwrap().position.is_none());
}

/// **0Uサイドマウントは格子の外に描くこと**（設計書12.9）。
async fn サイドマウントは格子の外に出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "sidedraw@example.com").await;
    let pdu = 機器(db, &場, "pdu-02", 1, "RackSide", None, "running").await;
    搭載行(db, &場, pdu.id, None, None, None, None).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &図のあて先(&場), &token).await;

    assert!(body.contains("0Uサイドマウント"));
    assert!(body.contains("pdu-02"));
    // 格子の中（rect）ではなくリストに出ていること
    assert!(!body.contains("class=\"rack-label\">pdu-02"));
}

/// 棚板の上の機器は位置を持たないこと（設計書12.3）。
async fn 棚板の上には位置を指定できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "shelf@example.com").await;
    let 棚板 = 機器(db, &場, "tray-01", 1, "RackU", None, "running").await;
    let nas = 機器(db, &場, "nas-01", 1, "RackU", None, "running").await;
    搭載行(db, &場, 棚板.id, Some(15), None, None, None).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 搭載(
        状態,
        &token,
        &場,
        nas.id,
        &[("position", "16"), ("host_device_id", &棚板.id.to_string())],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("位置は空にしてください"));

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 搭載(
        状態,
        &token,
        &場,
        nas.id,
        &[("host_device_id", &棚板.id.to_string())],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 行 = 現行の搭載(db, nas.id).await.unwrap();
    assert_eq!(行.host_device_id, Some(棚板.id));
    assert!(
        行.container_id.is_none(),
        "什器にも直接紐づいてしまっている"
    );
}

/// **棚板の上の機器は棚板の帯の中に描くこと**（設計書12.9）。
async fn 棚板の上の機器が帯の中に出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "shelfdraw@example.com").await;
    let 棚板 = 機器(db, &場, "tray-02", 1, "RackU", None, "running").await;
    let nas = 機器(db, &場, "nas-02", 1, "RackU", None, "running").await;
    搭載行(db, &場, 棚板.id, Some(15), None, None, None).await;
    棚の上に載せる(db, nas.id, 棚板.id).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &図のあて先(&場), &token).await;

    assert!(body.contains("rack-shelf-item"), "帯の中に描かれていない");
    assert!(body.contains("nas-02"));
}

/// **予約中の機器を区別して描くこと**（設計書11.6、12.9）。
///
/// 破線と淡色の両方を使い、凡例に文字のラベルも置く。
async fn 予約中は区別して描かれる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "planned@example.com").await;
    let 予約 = 機器(db, &場, "plan-01", 1, "RackU", None, "plan").await;
    搭載行(db, &場, 予約.id, Some(20), None, None, None).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &図のあて先(&場), &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("is-planned"), "予約中の印が付いていない");
    // **色や線種だけに意味を載せない**（12.9）
    assert!(body.contains("予約中"), "凡例に文字のラベルが無い");
}

/// **前面と背面を別の図として並べること**（設計書12.9）。
async fn 前面と背面が別の図になる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "panels@example.com").await;
    let f = 機器(db, &場, "front-02", 1, "RackU", None, "running").await;
    let r = 機器(db, &場, "rear-02", 1, "RackU", None, "running").await;
    搭載行(db, &場, f.id, Some(10), None, Some("Front"), None).await;
    搭載行(db, &場, r.id, Some(10), None, Some("Rear"), None).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &図のあて先(&場), &token).await;

    assert!(body.contains("前面"));
    assert!(body.contains("背面"));
    // どちらも1度ずつ。重ねていない
    assert_eq!(
        body.matches("front-02").count(),
        2,
        "一覧と図で1回ずつのはず"
    );
    assert_eq!(body.matches("rear-02").count(), 2);
}

/// 降ろすと行が閉じること。**消さない**（不変条件1）。
async fn 降ろすと行が閉じる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "unmount@example.com").await;
    let d = 機器(db, &場, "srv-x", 1, "RackU", None, "running").await;
    let 行 = 搭載行(db, &場, d.id, Some(7), None, None, None).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 送信(
        状態,
        &format!(
            "/projects/{}/containers/{}/unmount",
            場.project.id, 場.container.id
        ),
        &token,
        &[("mount_id", &行.to_string())],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 全部 = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(d.id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(全部.len(), 1, "行が消えている");
    assert!(全部[0].to_date.is_some());
}

/// **他プロジェクトの機器は搭載できないこと**（設計書3章）。
async fn 他プロジェクトの機器は搭載できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "outsider@example.com").await;
    let 他人の = プロジェクト(db, "他人のプロジェクト").await;
    let よそ = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        hostname: Set("their-01".to_owned()),
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
    所在(db, よそ.id, 他人の.id).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 搭載(状態, &token, &場, よそ.id, &[("position", "3")]).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("このプロジェクトにない機器"));
    assert!(現行の搭載(db, よそ.id).await.is_none());
}

/// Viewerは登録できないこと。
async fn 閲覧者は搭載できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "viewer-owner@example.com").await;
    let d = 機器(db, &場, "srv-v", 1, "RackU", None, "running").await;

    let 閲覧者 = 利用者(db, "viewer@example.com").await;
    メンバー(db, 閲覧者.id, 場.project.id, "Viewer").await;

    let (状態, token) = 認証済み(db, &閲覧者).await;
    let (status, _) = 搭載(状態, &token, &場, d.id, &[("position", "3")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 閲覧はできる
    let (状態, token) = 認証済み(db, &閲覧者).await;
    let (status, body) = 取得(状態, &図のあて先(&場), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("機器を搭載する"), "編集欄が出ている");
}

/// 机は格子を持たないこと（設計書12.9）。
async fn 机には格子を描かない(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "desk@example.com", "Operator").await;
    let desk = 什器(db, p.id, "作業机", "Desk", None, user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(
        状態,
        &format!("/projects/{}/containers/{}", p.id, desk.id),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("格子を持ちません"));
    assert!(!body.contains("rack-frame"), "格子が描かれている");
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
    container: mount_container::Model,
}

async fn 舞台(db: &DatabaseConnection, email: &str) -> 舞台情報 {
    let (user, project) = 準備(db, email, "Operator").await;
    let container = 什器(db, project.id, "Rack-01", "Rack", Some(42), user.id).await;
    舞台情報 {
        user,
        project,
        container,
    }
}

fn 図のあて先(場: &舞台情報) -> String {
    format!("/projects/{}/containers/{}", 場.project.id, 場.container.id)
}

async fn 搭載(
    state: AppState,
    token: &str,
    場: &舞台情報,
    device_id: i32,
    extra: &[(&str, &str)],
) -> (StatusCode, String) {
    let mut fields: Vec<(&str, String)> = vec![("device_id", device_id.to_string())];
    for (k, v) in extra {
        fields.push((k, (*v).to_owned()));
    }
    let borrowed: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
    送信(
        state,
        &format!(
            "/projects/{}/containers/{}/mounts",
            場.project.id, 場.container.id
        ),
        token,
        &borrowed,
    )
    .await
}

async fn 現行の搭載(db: &DatabaseConnection, device_id: i32) -> Option<device_mount::Model> {
    device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(device_id))
        .filter(device_mount::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
}

async fn 搭載行(
    db: &DatabaseConnection,
    場: &舞台情報,
    device_id: i32,
    position: Option<i32>,
    horizontal: Option<&str>,
    depth: Option<&str>,
    host: Option<i32>,
) -> i32 {
    device_mount::ActiveModel {
        device_id: Set(device_id),
        container_id: Set(host.is_none().then_some(場.container.id)),
        position: Set(position),
        horizontal_position: Set(horizontal.map(str::to_owned)),
        depth_position: Set(depth.map(str::to_owned)),
        host_device_id: Set(host),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

async fn 棚の上に載せる(db: &DatabaseConnection, device_id: i32, host: i32) {
    device_mount::ActiveModel {
        device_id: Set(device_id),
        container_id: Set(None),
        position: Set(None),
        horizontal_position: Set(None),
        depth_position: Set(None),
        host_device_id: Set(Some(host)),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

/// 型情報つきの機器を作る。所在もこのプロジェクトに置く。
async fn 機器(
    db: &DatabaseConnection,
    場: &舞台情報,
    hostname: &str,
    height_u: i32,
    mount_form: &str,
    rack_width: Option<&str>,
    status: &str,
) -> device::Model {
    let v = vendor::ActiveModel {
        name: Set(format!("ベンダー-{hostname}")),
        created_by: Set(場.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let m = chassis_model::ActiveModel {
        vendor_id: Set(v.id),
        model_name: Set(format!("型-{hostname}")),
        device_category: Set("Server".to_owned()),
        height_u: Set(height_u),
        mount_form: Set(mount_form.to_owned()),
        rack_width: Set(rack_width.map(str::to_owned)),
        created_by: Set(場.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let c = configuration::ActiveModel {
        chassis_model_id: Set(m.id),
        name: Set(format!("構成-{hostname}")),
        created_by: Set(場.user.id),
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
    .unwrap();

    所在(db, d.id, 場.project.id).await;
    d
}

async fn 所在(db: &DatabaseConnection, device_id: i32, project_id: i32) {
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

async fn 什器(
    db: &DatabaseConnection,
    project_id: i32,
    name: &str,
    container_type: &str,
    capacity: Option<i32>,
    created_by: i32,
) -> mount_container::Model {
    mount_container::ActiveModel {
        name: Set(name.to_owned()),
        container_type: Set(container_type.to_owned()),
        location_type: Set("Project".to_owned()),
        location_id: Set(project_id),
        capacity: Set(capacity),
        created_by: Set(created_by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 準備(
    db: &DatabaseConnection,
    email: &str,
    role: &str,
) -> (app_user::Model, project::Model) {
    let user = 利用者(db, email).await;
    let p = プロジェクト(db, &format!("{role}のプロジェクト")).await;
    メンバー(db, user.id, p.id, role).await;
    (user, p)
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

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(email.to_owned()),
        email: Set(email.to_owned()),
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
        user_id: Set(user_id),
        project_id: Set(project_id),
        role: Set(role.to_owned()),
        admin_rank: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 什器を登録できる);
        全検証!(@one $用意, $属性, 語彙外の種別は拒否される);
        全検証!(@one $用意, $属性, 機器を搭載できる);
        全検証!(@one $用意, $属性, 同じ位置には重ねられない);
        全検証!(@one $用意, $属性, 半幅なら左右に置ける);
        全検証!(@one $用意, $属性, 全幅の型は左右に置けない);
        全検証!(@one $用意, $属性, 前後同居は許すが警告する);
        全検証!(@one $用意, $属性, 収容能力の超過は警告に留まる);
        全検証!(@one $用意, $属性, 範囲外の機器も図に出る);
        全検証!(@one $用意, $属性, サイドマウントに位置は指定できない);
        全検証!(@one $用意, $属性, サイドマウントは格子の外に出る);
        全検証!(@one $用意, $属性, 棚板の上には位置を指定できない);
        全検証!(@one $用意, $属性, 棚板の上の機器が帯の中に出る);
        全検証!(@one $用意, $属性, 予約中は区別して描かれる);
        全検証!(@one $用意, $属性, 前面と背面が別の図になる);
        全検証!(@one $用意, $属性, 降ろすと行が閉じる);
        全検証!(@one $用意, $属性, 他プロジェクトの機器は搭載できない);
        全検証!(@one $用意, $属性, 閲覧者は搭載できない);
        全検証!(@one $用意, $属性, 机には格子を描かない);
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

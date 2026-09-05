//! カタログの統合の結合テスト（設計書18.5、23.9.4）。
//!
//! **統合は取り返しのつく操作ではない。**この画面が守るべきものを確かめる。
//!
//! - スペックが一致しないものを畳ませない（18.5）。**違うものを1つにすると
//!   集計が静かに狂う**
//! - ベンダー統合が一意制約に抵触するなら、**衝突を示して拒否する**（18.5）
//! - 吸収された側は削除せず、記録として残す（23.9.4）
//! - 権限は「いずれか1つ以上のプロジェクトでAdministrator」（18.5-4）

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
    app_user, chassis_model, configuration, configuration_part, part_catalog, part_instance,
    project, project_member, vendor,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 権限（設計書18.5-4）
// ---------------------------------------------------------------------------

/// **Operatorは統合できないこと**（18.5-4）。
///
/// 18.1のカタログ編集は「Operator以上」だが、統合はプロジェクト横断に影響する
/// ため一段上げてある。
async fn 操作者は統合できない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "op@example.com", "Operator").await;
    let a = ベンダー(db, "FUJITSU", user.id).await;
    let b = ベンダー(db, "Fujitsu", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = ベンダー統合(状態, &token, a.id, b.id).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(統合先(db, a.id).await.is_none());

    // 画面は開けるが、フォームが出ない
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, "/catalog/merge", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("管理者のロールを持つ利用者だけ"));
    assert!(!body.contains(r#"name="source_id""#), "フォームが出ている");
}

/// **System Adminは触れないこと**（設計書3章、18.1）。
async fn システム管理者は統合できない(db: &DatabaseConnection) {
    let sa = app_user::ActiveModel {
        name: Set("システム管理者".to_owned()),
        email: Set("sa@example.com".to_owned()),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(true),
        locale: Set("ja".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &sa).await;
    let (status, _) = 取得(状態, "/catalog/merge", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// ベンダーの統合（設計書18.5）
// ---------------------------------------------------------------------------

/// 参照を統合先へ付け替えること（18.3の各テーブル）。
async fn ベンダーの参照を付け替える(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "admin@example.com", "Administrator").await;
    let 誤 = ベンダー(db, "FUJITSU", user.id).await;
    let 正 = ベンダー(db, "Fujitsu", user.id).await;
    let m = 筐体モデル(db, 誤.id, "PRIMERGY RX2540 M7", user.id).await;
    let p = 部品(db, 誤.id, "CPU", "PY-CP62X8", None, user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ベンダー統合(状態, &token, 誤.id, 正.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("統合しました"), "{body}");

    assert_eq!(筐体モデルの取得(db, m.id).await.vendor_id, 正.id);
    assert_eq!(部品の取得(db, p.id).await.vendor_id, 正.id);
    // **削除ではなく転送先の設定**（23.9.4）
    assert_eq!(統合先(db, 誤.id).await, Some(正.id));
    assert!(ベンダーの取得(db, 誤.id).await.merged_at.is_some());
}

/// **吸収された側を一覧に出さないこと**（設計書23.9.4）。
///
/// 出すと同じベンダーが2行に見える。
async fn 吸収された側は一覧に出ない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "hide@example.com", "Administrator").await;
    let 誤 = ベンダー(db, "FUJITSU", user.id).await;
    let 正 = ベンダー(db, "Fujitsu", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    ベンダー統合(状態, &token, 誤.id, 正.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, "/catalog/vendors", &token).await;
    assert!(body.contains("Fujitsu"));
    assert!(!body.contains("FUJITSU"), "吸収された側が出ている");

    // 記録としては残る（23.9.4の「必ず記録を残す」）
    let (状態, token) = 認証済み(db, &user).await;
    let (_, 統合画面) = 取得(状態, "/catalog/merge", &token).await;
    assert!(統合画面.contains("FUJITSU"));
}

/// **下流に重複があるとベンダーを統合できないこと**（設計書18.5）。
///
/// `UNIQUE(vendor_id, model_name)` に抵触する。**どの行が衝突しているかを示す。**
async fn 下流が重複していると拒否される(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "conflict@example.com", "Administrator").await;
    let 誤 = ベンダー(db, "FUJITSU", user.id).await;
    let 正 = ベンダー(db, "Fujitsu", user.id).await;
    // 双方に同じ製品名がある
    筐体モデル(db, 誤.id, "PRIMERGY RX2540 M7", user.id).await;
    筐体モデル(db, 正.id, "PRIMERGY RX2540 M7", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ベンダー統合(状態, &token, 誤.id, 正.id).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("CHASSIS_MODEL"), "何が衝突したか示していない");
    assert!(body.contains("PRIMERGY RX2540 M7"), "どの行か示していない");
    assert!(統合先(db, 誤.id).await.is_none(), "統合されてしまっている");
}

/// 部品が重複している場合も同じく拒否すること（`UNIQUE(vendor_id, part_number)`）。
async fn 部品が重複していると拒否される(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "partconflict@example.com", "Administrator").await;
    let 誤 = ベンダー(db, "FUJITSU", user.id).await;
    let 正 = ベンダー(db, "Fujitsu", user.id).await;
    部品(db, 誤.id, "CPU", "PY-CP62X8", None, user.id).await;
    部品(db, 正.id, "CPU", "PY-CP62X8", None, user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ベンダー統合(状態, &token, 誤.id, 正.id).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("PART_CATALOG"));
    assert!(統合先(db, 誤.id).await.is_none());
}

/// **下流を先に統合すれば、ベンダーも統合できること**（設計書18.5の順序）。
///
/// これが「下流から上流へ」という順序の実際の姿である。
async fn 下流を片付ければ統合できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "order@example.com", "Administrator").await;
    let 誤 = ベンダー(db, "FUJITSU", user.id).await;
    let 正 = ベンダー(db, "Fujitsu", user.id).await;
    let a = 部品(db, 誤.id, "CPU", "PY-CP62X8", Some(8), user.id).await;
    let b = 部品(db, 正.id, "CPU", "PY-CP62X8", Some(8), user.id).await;

    // ①部品を統合する
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 部品統合(状態, &token, a.id, b.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("統合しました"), "{body}");

    // ②ベンダーを統合する。もう衝突しない
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ベンダー統合(状態, &token, 誤.id, 正.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("統合しました"), "{body}");
    assert_eq!(統合先(db, 誤.id).await, Some(正.id));
}

/// 同じものを選べないこと。
async fn 同じものは統合できない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "same@example.com", "Administrator").await;
    let v = ベンダー(db, "HPE", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ベンダー統合(状態, &token, v.id, v.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("同じものは選べません"));
}

/// **循環を作らせないこと**（設計書23.9.4）。
async fn 循環は拒否される(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "cycle@example.com", "Administrator").await;
    let a = ベンダー(db, "A社", user.id).await;
    let b = ベンダー(db, "B社", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    ベンダー統合(状態, &token, a.id, b.id).await;

    // B を A へ統合しようとすると循環する
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ベンダー統合(状態, &token, b.id, a.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("循環"), "{body}");
    assert!(統合先(db, b.id).await.is_none());
}

// ---------------------------------------------------------------------------
// 部品の統合（設計書18.5）
// ---------------------------------------------------------------------------

/// **スペックが一致しなければ統合しないこと**（設計書18.5）。
///
/// 違うものを1つにまとめると集計が静かに狂う。23.9.3が「構成が異なれば統合
/// しない」としたのと同じ判断である。
async fn スペックが違えば統合しない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "spec@example.com", "Administrator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let a = 部品(db, v.id, "CPU", "P24479-B21", Some(16), user.id).await;
    let b = 部品(db, v.id, "CPU", "P24480-B21", Some(32), user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 部品統合(状態, &token, a.id, b.id).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("スペックが一致しない"));
    assert!(部品の取得(db, a.id)
        .await
        .merged_into_part_catalog_id
        .is_none());
}

/// 実物と構成の参照を付け替えること（設計書18.5）。
async fn 部品の参照を付け替える(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "repoint@example.com", "Administrator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let a = 部品(db, v.id, "Memory", "P07640-B21", None, user.id).await;
    let b = 部品(db, v.id, "P07640-B21-DUP", "P07640-B21x", None, user.id).await;
    // 種別を揃える（上のヘルパは category に第3引数を使う）
    part_catalog::ActiveModel {
        id: Set(b.id),
        category: Set("Memory".to_owned()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let 実物 = part_instance::ActiveModel {
        part_catalog_id: Set(a.id),
        serial_number: Set(Some("SN-0001".to_owned())),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;
    let c = 構成(db, m.id, "標準構成", user.id).await;
    let 構成部品 = configuration_part::ActiveModel {
        configuration_id: Set(c.id),
        part_catalog_id: Set(a.id),
        quantity: Set(8),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 部品統合(状態, &token, a.id, b.id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("統合しました"), "{body}");

    assert_eq!(
        part_instance::Entity::find_by_id(実物.id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .part_catalog_id,
        b.id
    );
    assert_eq!(
        configuration_part::Entity::find_by_id(構成部品.id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .part_catalog_id,
        b.id
    );
    assert_eq!(
        部品の取得(db, a.id).await.merged_into_part_catalog_id,
        Some(b.id)
    );
}

/// **吸収された部品の詳細は開け、統合先へ誘導すること**（設計書23.9.4）。
///
/// ブックマークや過去の記録に残ったURLから辿り着いた人が「消えた」と誤解
/// しないようにするため。**削除ではないという設計が、画面上でもそう見える
/// 必要がある。**
async fn 吸収された部品の詳細は開ける(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "redirect@example.com", "Administrator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let a = 部品(db, v.id, "Memory", "OLD-001", None, user.id).await;
    let b = 部品(db, v.id, "Memory", "NEW-001", None, user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    部品統合(状態, &token, a.id, b.id).await;

    // 一覧からは消える
    let (状態, token) = 認証済み(db, &user).await;
    let (_, 一覧) = 取得(状態, "/catalog/parts", &token).await;
    assert!(!一覧.contains("OLD-001"), "吸収された側が一覧に出ている");

    // 詳細は開け、統合先が示される
    let (状態, token) = 認証済み(db, &user).await;
    let (status, 詳細) = 取得(状態, &format!("/catalog/parts/{}", a.id), &token).await;
    assert_eq!(status, StatusCode::OK, "詳細が開けない");
    assert!(詳細.contains("統合されました"));
    assert!(詳細.contains(&format!(r#"/catalog/parts/{}"#, b.id)));
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn ベンダー統合(
    state: AppState,
    token: &str,
    source: i32,
    target: i32,
) -> (StatusCode, String) {
    送信(
        state,
        "/catalog/merge/vendors",
        token,
        &[
            ("source_id", &source.to_string()),
            ("target_id", &target.to_string()),
        ],
    )
    .await
}

async fn 部品統合(
    state: AppState,
    token: &str,
    source: i32,
    target: i32,
) -> (StatusCode, String) {
    送信(
        state,
        "/catalog/merge/parts",
        token,
        &[
            ("source_id", &source.to_string()),
            ("target_id", &target.to_string()),
        ],
    )
    .await
}

async fn 統合先(db: &DatabaseConnection, id: i32) -> Option<i32> {
    ベンダーの取得(db, id).await.merged_into_vendor_id
}

async fn ベンダーの取得(db: &DatabaseConnection, id: i32) -> vendor::Model {
    vendor::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

async fn 部品の取得(db: &DatabaseConnection, id: i32) -> part_catalog::Model {
    part_catalog::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

async fn 筐体モデルの取得(db: &DatabaseConnection, id: i32) -> chassis_model::Model {
    chassis_model::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

async fn ベンダー(db: &DatabaseConnection, name: &str, by: i32) -> vendor::Model {
    vendor::ActiveModel {
        name: Set(name.to_owned()),
        retired_at: Set(None),
        merged_into_vendor_id: Set(None),
        merged_at: Set(None),
        created_by: Set(by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 筐体モデル(
    db: &DatabaseConnection,
    vendor_id: i32,
    model_name: &str,
    by: i32,
) -> chassis_model::Model {
    chassis_model::ActiveModel {
        vendor_id: Set(vendor_id),
        model_name: Set(model_name.to_owned()),
        device_category: Set("Server".to_owned()),
        height_u: Set(1),
        mount_form: Set("RackU".to_owned()),
        rack_width: Set(Some("Full".to_owned())),
        retired_at: Set(None),
        created_by: Set(by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 構成(
    db: &DatabaseConnection,
    chassis_model_id: i32,
    name: &str,
    by: i32,
) -> configuration::Model {
    configuration::ActiveModel {
        chassis_model_id: Set(chassis_model_id),
        name: Set(name.to_owned()),
        retired_at: Set(None),
        created_by: Set(by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 部品(
    db: &DatabaseConnection,
    vendor_id: i32,
    category: &str,
    part_number: &str,
    core_count: Option<i32>,
    by: i32,
) -> part_catalog::Model {
    part_catalog::ActiveModel {
        category: Set(category.to_owned()),
        vendor_id: Set(vendor_id),
        part_number: Set(part_number.to_owned()),
        core_count: Set(core_count),
        capacity_gb: Set(None),
        spec_json: Set("{}".to_owned()),
        retired_at: Set(None),
        merged_into_part_catalog_id: Set(None),
        merged_at: Set(None),
        created_by: Set(by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn メンバーの利用者(
    db: &DatabaseConnection,
    email: &str,
    role: &str,
) -> app_user::Model {
    let user = 利用者(db, email).await;
    let p = project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(None),
        name: Set(format!("{role}のプロジェクト")),
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
    .unwrap();

    project_member::ActiveModel {
        user_id: Set(user.id),
        project_id: Set(p.id),
        role: Set(role.to_owned()),
        admin_rank: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    user
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

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 操作者は統合できない);
        全検証!(@one $用意, $属性, システム管理者は統合できない);
        全検証!(@one $用意, $属性, ベンダーの参照を付け替える);
        全検証!(@one $用意, $属性, 吸収された側は一覧に出ない);
        全検証!(@one $用意, $属性, 下流が重複していると拒否される);
        全検証!(@one $用意, $属性, 部品が重複していると拒否される);
        全検証!(@one $用意, $属性, 下流を片付ければ統合できる);
        全検証!(@one $用意, $属性, 同じものは統合できない);
        全検証!(@one $用意, $属性, 循環は拒否される);
        全検証!(@one $用意, $属性, スペックが違えば統合しない);
        全検証!(@one $用意, $属性, 部品の参照を付け替える);
        全検証!(@one $用意, $属性, 吸収された部品の詳細は開ける);
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

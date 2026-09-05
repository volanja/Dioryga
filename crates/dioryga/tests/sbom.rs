//! SBOM取込の結合テスト（設計書9章）。
//!
//! **9.7の内容アドレス指定が実際に効いているか**を確かめるのが主眼である。
//! 同一構成の機器が何台あってもスナップショットと索引が増えない、という
//! 性質は10,000台規模（17.2）の成立条件そのものであり、壊れても画面上は
//! 何も起きない。
//!
//! あわせて9.6が定めた次の点も確かめる。
//!
//! - 内容が変わっていなければ差分は0件、索引も作らない
//! - 取込は `SOFTWARE_INSTANCE` も `VENDOR` も作らない
//! - 索引の作成は**新規の `content_hash` のときだけ**
//! - 行ごとの監査ログを書かない（24.4）

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
    app_user, audit_log, device, device_assignment, project, project_member, sbom_component_change,
    sbom_component_index, sbom_import, sbom_snapshot, software_catalog, vendor,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;
use tower::ServiceExt;

const 境界: &str = "----------------------------dioryga";

/// CycloneDX。コンポーネント2件。
const CDX_V1: &str = r#"{
  "bomFormat": "CycloneDX",
  "specVersion": "1.5",
  "components": [
    {"type":"library","name":"openssl","version":"3.0.13","purl":"pkg:generic/openssl@3.0.13"},
    {"type":"library","name":"zlib","version":"1.3","purl":"pkg:generic/zlib@1.3"}
  ]
}"#;

/// openssl が上がり、zlib が消え、curl が入った版。
const CDX_V2: &str = r#"{
  "bomFormat": "CycloneDX",
  "specVersion": "1.5",
  "components": [
    {"type":"library","name":"openssl","version":"3.0.14","purl":"pkg:generic/openssl@3.0.14"},
    {"type":"library","name":"curl","version":"8.6.0","purl":"pkg:generic/curl@8.6.0"}
  ]
}"#;

/// 出力順だけが違う CDX_V1。**同じスナップショットになるはず**（9.7）。
const CDX_V1_順序違い: &str = r#"{
  "bomFormat": "CycloneDX",
  "specVersion": "1.5",
  "components": [
    {"type":"library","name":"zlib","version":"1.3","purl":"pkg:generic/zlib@1.3"},
    {"type":"library","name":"openssl","version":"3.0.13","purl":"pkg:generic/openssl@3.0.13"}
  ]
}"#;

// ---------------------------------------------------------------------------
// 内容アドレス指定（設計書9.7）
// ---------------------------------------------------------------------------

/// 取り込めること。索引も作られること（9.6の手順5）。
async fn 取り込める(db: &DatabaseConnection) {
    let 場 = 舞台(db, "import@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取込(状態, &token, &場, 場.device_a, CDX_V1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("2 件のコンポーネント"), "{body}");

    assert_eq!(スナップショット数(db).await, 1);
    assert_eq!(索引数(db).await, 2);

    let imports = 取込一覧(db).await;
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].source_format, "CycloneDX");
    // 最新の観測結果（9.5）
    assert!(imports[0].superseded_at.is_none());

    // **初回は全件が `added`。**直前の観測が無いので、そこにあるもの全部が
    // 「増えた」ことになる
    let changes = 最新の差分(db).await;
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| c.change_type == "added"));
}

/// **同一構成の2台目でスナップショットも索引も増えないこと**（設計書9.7、9.8）。
///
/// ゴールデンイメージから構築された機器のSBOMは完全に一致する。ここが崩れると
/// 10,000台規模で保存量が台数に比例してしまう。
async fn 同一構成の二台目はスナップショットを共有する(
    db: &DatabaseConnection,
) {
    let 場 = 舞台(db, "share@example.com").await;

    for d in [場.device_a, 場.device_b] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, _) = 取込(状態, &token, &場, d, CDX_V1).await;
        assert_eq!(status, StatusCode::OK);
    }

    assert_eq!(
        スナップショット数(db).await,
        1,
        "スナップショットが増えている"
    );
    assert_eq!(索引数(db).await, 2, "索引が台数ぶんに増えている");
    // 取込の記録は台数ぶんある。共有されているのはスナップショットだけ
    assert_eq!(取込一覧(db).await.len(), 2);

    // 2台目は「再利用した」と伝える
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取込(状態, &token, &場, 場.device_b, CDX_V1).await;
    assert!(body.contains("前回から構成に変化はありません"), "{body}");
}

/// **出力順が違っても同じスナップショットになること**（設計書9.6の手順1）。
///
/// 順序が揺れると内容アドレス指定が成立しない。
async fn 出力順が違っても共有される(db: &DatabaseConnection) {
    let 場 = 舞台(db, "order@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    取込(状態, &token, &場, 場.device_a, CDX_V1).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    取込(状態, &token, &場, 場.device_b, CDX_V1_順序違い).await;

    assert_eq!(スナップショット数(db).await, 1);
}

// ---------------------------------------------------------------------------
// 差分（設計書9.6）
// ---------------------------------------------------------------------------

/// 前回からの差分を記録すること。
async fn 差分を記録する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "diff@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    取込(状態, &token, &場, 場.device_a, CDX_V1).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取込(状態, &token, &場, 場.device_a, CDX_V2).await;

    // openssl が version_changed、zlib が removed、curl が added。
    // **最新の取込に絞る**——初回の取込は全件が added になる（前が空のため）
    let changes = 最新の差分(db).await;
    assert_eq!(changes.len(), 3, "{body}");

    let 探す = |kind: &str| changes.iter().find(|c| c.change_type == kind).cloned();
    let 版 = 探す("version_changed").unwrap();
    assert_eq!(版.name, "openssl");
    assert_eq!(版.version_from.as_deref(), Some("3.0.13"));
    assert_eq!(版.version_to.as_deref(), Some("3.0.14"));
    assert_eq!(探す("removed").unwrap().name, "zlib");
    assert_eq!(探す("added").unwrap().name, "curl");

    // 画面に差分が出る
    assert!(body.contains("version_changed"));
    assert!(body.contains("zlib"));
}

/// **直前の行が閉じ、新しい行が最新になること**（設計書9.6の手順4）。
async fn 直前の取込が閉じる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "supersede@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    取込(状態, &token, &場, 場.device_a, CDX_V1).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    取込(状態, &token, &場, 場.device_a, CDX_V2).await;

    let imports = 取込一覧(db).await;
    assert_eq!(imports.len(), 2);
    let 現行: Vec<_> = imports
        .iter()
        .filter(|i| i.superseded_at.is_none())
        .collect();
    assert_eq!(現行.len(), 1, "最新が1件でない");
    // 新しいほうが最新
    assert_eq!(現行[0].id, imports.iter().map(|i| i.id).max().unwrap());
}

/// **内容が同じなら差分は0件で、索引も作らないこと**（設計書9.6の手順3、5）。
async fn 変化が無ければ差分も索引も作らない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "same@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    取込(状態, &token, &場, 場.device_a, CDX_V1).await;
    let 索引 = 索引数(db).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取込(状態, &token, &場, 場.device_a, CDX_V1).await;

    assert!(body.contains("構成に変化はありません"));
    assert_eq!(最新の差分(db).await.len(), 0);
    assert_eq!(索引数(db).await, 索引, "索引が二重に作られている");
    // 取込の記録だけは増える（9.6の手順3）
    assert_eq!(取込一覧(db).await.len(), 2);
}

// ---------------------------------------------------------------------------
// 取込は何も自動生成しない（設計書9.6、18.3）
// ---------------------------------------------------------------------------

/// **`SOFTWARE_CATALOG` も `VENDOR` も作らないこと**（設計書9.2、18.3）。
///
/// 10,000台規模では数千種類のサプライヤ名が流れ込み、18章で表記ゆれを解消する
/// ために作ったマスタが逆に汚染される。
async fn 取込はカタログもベンダーも作らない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "noauto@example.com").await;
    let json = r#"{"bomFormat":"CycloneDX","components":[
      {"type":"library","name":"openssl","version":"3.0.13",
       "supplier":{"name":"OpenSSL Project"}}]}"#;

    let (状態, token) = 認証済み(db, &場.user).await;
    取込(状態, &token, &場, 場.device_a, json).await;

    assert!(
        vendor::Entity::find().all(db).await.unwrap().is_empty(),
        "VENDOR が作られている"
    );
    assert!(
        software_catalog::Entity::find()
            .all(db)
            .await
            .unwrap()
            .is_empty(),
        "SOFTWARE_CATALOG が作られている"
    );

    // サプライヤ名はスナップショット内に残る
    let s = sbom_snapshot::Entity::find().all(db).await.unwrap();
    let 展開 = zstd::decode_all(&s[0].content[..]).unwrap();
    assert!(String::from_utf8_lossy(&展開).contains("OpenSSL Project"));
}

/// **行ごとの監査ログを書かないこと**（設計書24.4）。
///
/// 索引は1回の取込で数千行入りうる。
async fn 索引の監査ログを書かない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "audit@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    取込(状態, &token, &場, 場.device_a, CDX_V1).await;

    let logs = audit_log::Entity::find()
        .filter(audit_log::Column::TableName.is_in([
            "sbom_component_index",
            "sbom_snapshot",
            "sbom_import",
            "sbom_component_change",
        ]))
        .all(db)
        .await
        .unwrap();
    assert!(
        logs.is_empty(),
        "SBOM取込で監査ログが {} 行できている",
        logs.len()
    );

    // 追跡は IMPORT_RUN が担う（23.7）
    let runs = entity::import_run::Entity::find().all(db).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].kind, "sbom");
}

// ---------------------------------------------------------------------------
// 入口の検証
// ---------------------------------------------------------------------------

/// **読めなかった理由をそのまま出すこと。**
///
/// 「失敗しました」だけでは、形式が違うのか壊れているのか分からない。
async fn 読めないものは理由を出す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "badfile@example.com").await;

    for (中身, 期待) in [
        ("これはJSONではない", "JSONとして読めません"),
        (r#"{"foo":1}"#, "CycloneDX でも SPDX でもありません"),
        (
            r#"{"bomFormat":"CycloneDX","components":[]}"#,
            "コンポーネントが1件もありません",
        ),
    ] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = 取込(状態, &token, &場, 場.device_a, 中身).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(期待), "{中身} → {body}");
    }

    assert_eq!(取込一覧(db).await.len(), 0);
}

/// SPDXも取り込めること（設計書9.1）。
async fn spdxも取り込める(db: &DatabaseConnection) {
    let 場 = 舞台(db, "spdx@example.com").await;
    let json = r#"{"spdxVersion":"SPDX-2.3","packages":[
      {"name":"openssl","versionInfo":"3.0.13",
       "externalRefs":[{"referenceType":"purl","referenceLocator":"pkg:generic/openssl@3.0.13"}]}]}"#;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 取込(状態, &token, &場, 場.device_a, json).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(取込一覧(db).await[0].source_format, "SPDX");
}

/// **Viewerは取り込めないこと。**
async fn 閲覧者は取り込めない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "viewer-owner@example.com").await;
    let 閲覧者 = 利用者(db, "viewer@example.com").await;
    メンバー(db, 閲覧者.id, 場.project_id, "Viewer").await;

    let (状態, token) = 認証済み(db, &閲覧者).await;
    let (status, _) = 取込(状態, &token, &場, 場.device_a, CDX_V1).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 閲覧はできる
    let (状態, token) = 認証済み(db, &閲覧者).await;
    let (status, body) = 取得(
        状態,
        &format!("/projects/{}/devices/{}/sbom", 場.project_id, 場.device_a),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("SBOMを取り込む"), "取込欄が出ている");
}

/// **他プロジェクトの機器のSBOMは見られないこと**（設計書3章、9.8）。
async fn 他プロジェクトの機器は見られない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "outsider@example.com").await;
    let 他人の = プロジェクト(db, "他人のプロジェクト").await;
    let よそ = 機器(db, "their-01").await;
    所在(db, よそ.id, 他人の.id).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 取得(
        状態,
        &format!("/projects/{}/devices/{}/sbom", 場.project_id, よそ.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project_id: i32,
    device_a: i32,
    device_b: i32,
}

async fn 舞台(db: &DatabaseConnection, email: &str) -> 舞台情報 {
    let user = 利用者(db, email).await;
    let p = プロジェクト(db, "基幹刷新").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let a = 機器(db, "golden-a").await;
    let b = 機器(db, "golden-b").await;
    所在(db, a.id, p.id).await;
    所在(db, b.id, p.id).await;

    舞台情報 {
        user,
        project_id: p.id,
        device_a: a.id,
        device_b: b.id,
    }
}

fn multipart(content: &str) -> String {
    format!(
        "--{境界}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"sbom.json\"\r\n\
         Content-Type: application/json\r\n\r\n{content}\r\n\
         --{境界}--\r\n"
    )
}

async fn 取込(
    state: AppState,
    token: &str,
    場: &舞台情報,
    device_id: i32,
    content: &str,
) -> (StatusCode, String) {
    let csrf = dioryga::auth::csrf::derive(token);
    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{}/devices/{device_id}/sbom",
                    場.project_id
                ))
                .header(header::COOKIE, cookie_header(token))
                .header(dioryga::auth::csrf::HEADER_NAME, csrf)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={境界}"),
                )
                .body(Body::from(multipart(content)))
                .unwrap(),
        )
        .await
        .unwrap();
    分解(res).await
}

async fn スナップショット数(db: &DatabaseConnection) -> usize {
    sbom_snapshot::Entity::find().all(db).await.unwrap().len()
}

async fn 索引数(db: &DatabaseConnection) -> usize {
    sbom_component_index::Entity::find()
        .all(db)
        .await
        .unwrap()
        .len()
}

async fn 取込一覧(db: &DatabaseConnection) -> Vec<sbom_import::Model> {
    sbom_import::Entity::find()
        .order_by_asc(sbom_import::Column::Id)
        .all(db)
        .await
        .unwrap()
}

/// 最新の取込（`superseded_at IS NULL`）の差分だけを返す。
///
/// **初回の取込は全件が `added` になる**（直前が空のため）。合算すると
/// 「今回何が変わったか」を確かめられない。
async fn 最新の差分(db: &DatabaseConnection) -> Vec<sbom_component_change::Model> {
    let 最新 = sbom_import::Entity::find()
        .filter(sbom_import::Column::SupersededAt.is_null())
        .order_by_desc(sbom_import::Column::Id)
        .one(db)
        .await
        .unwrap()
        .expect("取込が無い");

    sbom_component_change::Entity::find()
        .filter(sbom_component_change::Column::SbomImportId.eq(最新.id))
        .order_by_asc(sbom_component_change::Column::Id)
        .all(db)
        .await
        .unwrap()
}

#[allow(dead_code)]
async fn 差分一覧(db: &DatabaseConnection) -> Vec<sbom_component_change::Model> {
    sbom_component_change::Entity::find()
        .order_by_asc(sbom_component_change::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 機器(db: &DatabaseConnection, hostname: &str) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        hostname: Set(hostname.to_owned()),
        device_type: Set("Physical".to_owned()),
        power_watt: Set(350),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
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
        全検証!(@one $用意, $属性, 取り込める);
        全検証!(@one $用意, $属性, 同一構成の二台目はスナップショットを共有する);
        全検証!(@one $用意, $属性, 出力順が違っても共有される);
        全検証!(@one $用意, $属性, 差分を記録する);
        全検証!(@one $用意, $属性, 直前の取込が閉じる);
        全検証!(@one $用意, $属性, 変化が無ければ差分も索引も作らない);
        全検証!(@one $用意, $属性, 取込はカタログもベンダーも作らない);
        全検証!(@one $用意, $属性, 索引の監査ログを書かない);
        全検証!(@one $用意, $属性, 読めないものは理由を出す);
        全検証!(@one $用意, $属性, spdxも取り込める);
        全検証!(@one $用意, $属性, 閲覧者は取り込めない);
        全検証!(@one $用意, $属性, 他プロジェクトの機器は見られない);
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

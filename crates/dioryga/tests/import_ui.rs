//! 一括取込画面の結合テスト（設計書16.1のB領域、23.6）。
//!
//! **23.6が求める2段階が、画面の上でも成立しているか**を確かめる。
//! 差分を見せておいて別の内容を反映する、エラーがあるのに反映できる、という
//! 事故はいずれも取り返しがつかない。

mod support;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::import::StagedUploads;
use dioryga::server::{router, AppState};
use entity::{app_user, device, import_run, project, project_member};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use std::sync::Arc;
use tower::ServiceExt;

const 境界: &str = "----------------------------dioryga";

/// multipart の本文を組み立てる。ブラウザが送る形に合わせる。
fn multipart(filename: &str, content: &str, fields: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (name, value) in fields {
        body.push_str(&format!("--{境界}\r\n"));
        body.push_str(&format!(
            "Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!("--{境界}\r\n"));
    body.push_str(&format!(
        "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: text/csv\r\n\r\n{content}\r\n"
    ));
    body.push_str(&format!("--{境界}--\r\n"));
    body
}

const CSV: &str = "uid,external_id,hostname,serial_number,device_type,power_watt,status\n\
     ,SV-0001,web01,JP123,Physical,450,running\n";

// ---------------------------------------------------------------------------
// 2段階（設計書23.6）
// ---------------------------------------------------------------------------

/// **アップロードしただけでは書き込まないこと**（設計書23.6）。
async fn アップロードだけでは反映されない(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "stage@example.com", "Operator").await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/import", p.id),
        &token,
        multipart("devices.csv", CSV, &[("match_on", "serial_number")]),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("新規 1 件"), "差分が表示されていません");
    assert!(body.contains("まだ何も書き込んでいません"));

    assert_eq!(台数(db).await, 0, "アップロードだけで書き込まれています");
    assert_eq!(取込回数(db).await, 0);
}

/// 差分を見てから反映できること。
async fn 差分を見てから反映できる(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "apply@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 送信(
        状態,
        &format!("/projects/{}/import", p.id),
        &token,
        multipart("devices.csv", CSV, &[("match_on", "serial_number")]),
    )
    .await;

    let 預かり = 預かりトークン(&body);

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = フォーム送信(
        状態,
        &format!("/projects/{}/import/apply", p.id),
        &token,
        &[("token", &預かり)],
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(台数(db).await, 1);

    // 取込の記録が残り、件数が入ること（設計書23.7）
    let run = import_run::Entity::find()
        .one(db)
        .await
        .unwrap()
        .expect("IMPORT_RUNが記録されていません");
    assert_eq!(run.created_count, 1);
    assert_eq!(run.project_id, Some(p.id));

    // **取込では行ごとの監査ログを書かない**（24.4）
    assert!(entity::audit_log::Entity::find()
        .all(db)
        .await
        .unwrap()
        .is_empty());
}

/// **反映は一度きりであること。**
///
/// 同じトークンで二度流せると、履歴と IMPORT_RUN が二重に入る。
async fn 同じ預かりで二度は反映できない(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "twice@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 送信(
        状態,
        &format!("/projects/{}/import", p.id),
        &token,
        multipart("devices.csv", CSV, &[("match_on", "serial_number")]),
    )
    .await;
    let 預かり = 預かりトークン(&body);

    for _ in 0..2 {
        let (状態, token) = 認証済み(db, &user).await;
        フォーム送信(
            状態,
            &format!("/projects/{}/import/apply", p.id),
            &token,
            &[("token", &預かり)],
        )
        .await;
    }

    assert_eq!(台数(db).await, 1);
    assert_eq!(取込回数(db).await, 1, "二度目の反映が通っています");
}

/// **エラーがあれば反映のボタンを出さないこと**（設計書23.6）。
async fn エラーがあれば反映できない(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "err@example.com", "Operator").await;
    let 壊れたcsv = "uid,hostname,device_type\n,web01,でたらめ\n";

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/import", p.id),
        &token,
        multipart("devices.csv", 壊れたcsv, &[]),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("エラーがあるため反映できません"));
    assert!(
        !body.contains("この内容で反映する"),
        "反映のボタンが出ています"
    );
}

/// **他人の預かりは反映できないこと。**
///
/// トークンが漏れても、別の利用者の取込を実行させない。
async fn 他人の預かりは反映できない(db: &DatabaseConnection) {
    let (本人, p) = 準備(db, "owner@example.com", "Operator").await;
    let 別人 = 利用者(db, "other@example.com").await;
    メンバー(db, 別人.id, p.id, "Operator").await;

    let (状態, token) = 認証済み(db, &本人).await;
    let (_, body) = 送信(
        状態,
        &format!("/projects/{}/import", p.id),
        &token,
        multipart("devices.csv", CSV, &[("match_on", "serial_number")]),
    )
    .await;
    let 預かり = 預かりトークン(&body);

    let (状態, token) = 認証済み(db, &別人).await;
    let (status, body) = フォーム送信(
        状態,
        &format!("/projects/{}/import/apply", p.id),
        &token,
        &[("token", &預かり)],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("期限が切れたか"));
    assert_eq!(台数(db).await, 0, "他人の預かりが反映されました");
}

// ---------------------------------------------------------------------------
// 権限（設計書23.5）
// ---------------------------------------------------------------------------

/// **取込にはOperator以上が要ること。**ViewerとApproverは入れない。
async fn 閲覧のみのロールは取り込めない(db: &DatabaseConnection) {
    for role in ["Viewer", "Approver"] {
        let (user, p) = 準備(db, &format!("{role}@example.com"), role).await;

        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 取得(状態, &format!("/projects/{}/import", p.id), &token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{role} が画面に入れました");

        // 画面を隠すだけでは足りない。送信も拒否する
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 送信(
            状態,
            &format!("/projects/{}/import", p.id),
            &token,
            multipart("devices.csv", CSV, &[]),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{role} が送信できました");
    }
}

/// 非メンバーは入れないこと。
async fn 非メンバーは取り込めない(db: &DatabaseConnection) {
    let user = 利用者(db, "outsider@example.com").await;
    let p = プロジェクト(db, "よそのプロジェクト").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 取得(状態, &format!("/projects/{}/import", p.id), &token).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// **素のHTMLフォームだけで取り込めること**（不変条件9、設計書20.5）。
///
/// `enctype="multipart/form-data"` のフォームは**CSRFヘッダーを付けられない。**
/// hidden fieldだけで通らないと、JavaScriptを使わない画面が403になる。
///
/// P5-1で `application/x-www-form-urlencoded` について同じ穴を塞いだが、
/// **multipartは塞がっていなかった**——取込UIはブラウザから使えていなかった。
async fn ヘッダー無しでも取り込める(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "noheader@example.com", "Operator").await;
    let (状態, token) = 認証済み(db, &user).await;

    let csrf = dioryga::auth::csrf::derive(&token);
    let body = multipart(
        "devices.csv",
        CSV,
        &[
            (dioryga::auth::csrf::FIELD_NAME, &csrf),
            ("match_on", "serial_number"),
        ],
    );

    let res = router(状態)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/projects/{}/import", p.id))
                .header(header::COOKIE, cookie_header(&token))
                // **ヘッダーは付けない。**ブラウザのフォームと同じ条件にする
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={境界}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    let (status, body) = 分解(res).await;
    assert_eq!(status, StatusCode::OK, "CSRFで弾かれている");
    assert!(body.contains("新規 1 件"), "{body}");
}

/// CSRFトークンが違えば弾くこと（multipartでも）。
async fn multipartでも不正なトークンは弾く(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "badtoken@example.com", "Operator").await;
    let (状態, token) = 認証済み(db, &user).await;

    let body = multipart(
        "devices.csv",
        CSV,
        &[
            (dioryga::auth::csrf::FIELD_NAME, "でたらめなトークン"),
            ("match_on", "serial_number"),
        ],
    );

    let res = router(状態)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/projects/{}/import", p.id))
                .header(header::COOKIE, cookie_header(&token))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={境界}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(分解(res).await.0, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// レポート画面の hidden field から預かりトークンを取り出す。
fn 預かりトークン(body: &str) -> String {
    let 印 = r#"name="token" value=""#;
    let 開始 = body
        .find(印)
        .unwrap_or_else(|| panic!("預かりトークンが画面にありません: {body}"))
        + 印.len();
    let 終了 = 開始 + body[開始..].find('"').unwrap();
    body[開始..終了].to_owned()
}

async fn 台数(db: &DatabaseConnection) -> usize {
    device::Entity::find().all(db).await.unwrap().len()
}

async fn 取込回数(db: &DatabaseConnection) -> usize {
    import_run::Entity::find().all(db).await.unwrap().len()
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

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

/// **同じ預かり庫を使い回す。**リクエストごとに作り直すと、
/// ドライランで預けた内容を反映時に見つけられない。
async fn 認証済み(db: &DatabaseConnection, user: &app_user::Model) -> (AppState, String) {
    use std::sync::OnceLock;
    static 預かり庫: OnceLock<StagedUploads> = OnceLock::new();

    let config = 設定();
    let (setup, _) = SetupState::initialize(db).await.unwrap();
    let state = AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
        staged: 預かり庫.get_or_init(StagedUploads::default).clone(),
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

/// multipart で送る。CSRFトークンはヘッダーに載せる
/// （multipart の本文はCSRF検証が読まないため）。
async fn 送信(state: AppState, uri: &str, token: &str, body: String) -> (StatusCode, String) {
    let csrf = dioryga::auth::csrf::derive(token);
    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie_header(token))
                .header(dioryga::auth::csrf::HEADER_NAME, csrf)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={境界}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    分解(res).await
}

async fn フォーム送信(
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
        name: Set("取込".to_owned()),
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
        全検証!(@one $用意, $属性, アップロードだけでは反映されない);
        全検証!(@one $用意, $属性, ヘッダー無しでも取り込める);
        全検証!(@one $用意, $属性, multipartでも不正なトークンは弾く);
        全検証!(@one $用意, $属性, 差分を見てから反映できる);
        全検証!(@one $用意, $属性, 同じ預かりで二度は反映できない);
        全検証!(@one $用意, $属性, エラーがあれば反映できない);
        全検証!(@one $用意, $属性, 他人の預かりは反映できない);
        全検証!(@one $用意, $属性, 閲覧のみのロールは取り込めない);
        全検証!(@one $用意, $属性, 非メンバーは取り込めない);
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

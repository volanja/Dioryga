//! 認証・認可の結合テスト（設計書3章、20.6、20.10）。

mod support;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use dioryga::auth::authorization::{self, ADMINISTRATOR, APPROVER, OPERATOR, VIEWER};
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, project, project_member};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use std::sync::Arc;
use tower::ServiceExt;

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

async fn 状態(db: &DatabaseConnection) -> AppState {
    let config = 設定();
    // セットアップ済みの状態にする（利用者を作った後に呼ぶこと）
    let (setup, _) = SetupState::initialize(db).await.unwrap();
    AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
        staged: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// 認可（関数レベル）
// ---------------------------------------------------------------------------

/// **System Adminはプロジェクト内データに触れない**（設計書3章）。
///
/// 一般的なRBACと異なる意図的な逆転制約であり、ロールを持っていても拒否する。
async fn system_adminはロールを持っていても拒否される(db: &DatabaseConnection) {
    let admin = 利用者(db, "sysadmin@example.com", true).await;
    let p = プロジェクト(db, "検証").await;
    // あえて最上位のロールを与える
    メンバー(db, admin.id, p.id, ADMINISTRATOR).await;

    let 結果 =
        authorization::require_project_role(db, &admin, p.id, &[VIEWER, ADMINISTRATOR]).await;

    assert!(
        matches!(結果, Err(authorization::AuthzError::Denied)),
        "System Adminがプロジェクトデータへアクセスできてしまいました"
    );
}

/// 兼務しているロールのいずれかが要件を満たせば通ること（設計書5章）。
async fn 兼務しているロールのいずれかで通る(db: &DatabaseConnection) {
    let user = 利用者(db, "multi@example.com", false).await;
    let p = プロジェクト(db, "兼務検証").await;
    メンバー(db, user.id, p.id, OPERATOR).await;
    メンバー(db, user.id, p.id, APPROVER).await;

    // Approver を要求 → 兼務しているので通る
    assert!(
        authorization::require_project_role(db, &user, p.id, &[APPROVER])
            .await
            .is_ok()
    );
    // Administrator を要求 → 持っていないので拒否
    assert!(
        authorization::require_project_role(db, &user, p.id, &[ADMINISTRATOR])
            .await
            .is_err()
    );
}

/// 別プロジェクトのロールでは通らないこと。
async fn 別プロジェクトのロールでは通らない(db: &DatabaseConnection) {
    let user = 利用者(db, "other@example.com", false).await;
    let a = プロジェクト(db, "A").await;
    let b = プロジェクト(db, "B").await;
    メンバー(db, user.id, a.id, ADMINISTRATOR).await;

    assert!(
        authorization::require_project_role(db, &user, a.id, &[ADMINISTRATOR])
            .await
            .is_ok()
    );
    assert!(
        authorization::require_project_role(db, &user, b.id, &[ADMINISTRATOR])
            .await
            .is_err()
    );
}

/// 無効化された利用者は拒否されること（設計書20.11）。
async fn 無効化された利用者は拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "disabled-authz@example.com", false).await;
    let p = プロジェクト(db, "無効化検証").await;
    メンバー(db, user.id, p.id, ADMINISTRATOR).await;

    let mut active: app_user::ActiveModel = user.clone().into();
    active.disabled_at = Set(Some(Utc::now()));
    let 無効化後 = active.update(db).await.unwrap();

    assert!(
        authorization::require_project_role(db, &無効化後, p.id, &[ADMINISTRATOR])
            .await
            .is_err()
    );
}

/// カタログはいずれかのプロジェクトでOperator以上なら編集できる（設計書18.1）。
/// **System Adminは編集できない。**
async fn カタログ編集はoperator以上でsystem_adminは不可(db: &DatabaseConnection) {
    let operator = 利用者(db, "cat-op@example.com", false).await;
    let viewer = 利用者(db, "cat-view@example.com", false).await;
    let admin = 利用者(db, "cat-sys@example.com", true).await;
    let p = プロジェクト(db, "カタログ検証").await;

    メンバー(db, operator.id, p.id, OPERATOR).await;
    メンバー(db, viewer.id, p.id, VIEWER).await;

    assert!(authorization::require_catalog_editor(db, &operator)
        .await
        .is_ok());
    assert!(authorization::require_catalog_editor(db, &viewer)
        .await
        .is_err());
    assert!(
        authorization::require_catalog_editor(db, &admin)
            .await
            .is_err(),
        "System Adminがカタログを編集できてしまいました"
    );
}

// ---------------------------------------------------------------------------
// ミドルウェア（HTTPレベル）
// ---------------------------------------------------------------------------

/// 未認証で保護された経路にアクセスするとログインへ誘導されること。
async fn 未認証は誘導される(db: &DatabaseConnection) {
    let _ = 利用者(db, "seed@example.com", false).await;
    let app = router(状態(db).await);

    let res = app
        .oneshot(
            Request::builder()
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/login");
}

/// ログイン画面自体は認証不要であること（許可リスト）。
async fn ログイン画面は認証不要(db: &DatabaseConnection) {
    let _ = 利用者(db, "seed2@example.com", false).await;
    let app = router(状態(db).await);

    let res = app
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
}

/// **System Adminは /projects へアクセスできない**（設計書3章）。
async fn system_adminはprojectsへアクセスできない(db: &DatabaseConnection) {
    let admin = 利用者(db, "guard-sys@example.com", true).await;
    let state = 状態(db).await;
    let token = セッション(db, admin.id, &state).await;

    let res = router(state)
        .oneshot(
            Request::builder()
                .uri("/projects")
                .header(header::COOKIE, cookie_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "System Adminがプロジェクト領域に入れてしまいました"
    );
}

/// 一般の利用者は /projects へアクセスできること。
async fn 一般利用者はprojectsへアクセスできる(db: &DatabaseConnection) {
    let user = 利用者(db, "guard-user@example.com", false).await;
    let state = 状態(db).await;
    let token = セッション(db, user.id, &state).await;

    let res = router(state)
        .oneshot(
            Request::builder()
                .uri("/projects")
                .header(header::COOKIE, cookie_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
}

/// 失効したセッションでは通らないこと。
async fn 失効したセッションは拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "revoked@example.com", false).await;
    let state = 状態(db).await;
    let now = Utc::now();
    let (model, token) = session::create(db, user.id, "127.0.0.1", "t", &state.config.session, now)
        .await
        .unwrap();
    session::revoke(db, model, now).await.unwrap();

    let res = router(state)
        .oneshot(
            Request::builder()
                .uri("/projects")
                .header(header::COOKIE, cookie_header(token.as_str()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::SEE_OTHER);
}

/// パスワード変更を強制されている間は他の画面へ進めないこと（設計書20.6）。
async fn パスワード変更が必要なら誘導される(db: &DatabaseConnection) {
    let user = 利用者(db, "mustchange@example.com", false).await;
    let mut active: app_user::ActiveModel = user.clone().into();
    active.must_change_password = Set(true);
    let user = active.update(db).await.unwrap();

    let state = 状態(db).await;
    let token = セッション(db, user.id, &state).await;

    let res = router(state)
        .oneshot(
            Request::builder()
                .uri("/projects")
                .header(header::COOKIE, cookie_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/account/password");
}

/// **パスワード変更の強制中でも静的アセットは通ること。**
///
/// 通さないとパスワード変更画面が素のHTMLになる。画面自体は出るので、
/// 見た目を確かめない限り気付かない。
async fn パスワード変更の強制中でも静的アセットは通る(
    db: &DatabaseConnection,
) {
    let user = 利用者(db, "mustchange-css@example.com", false).await;
    let mut active: app_user::ActiveModel = user.clone().into();
    active.must_change_password = Set(true);
    let user = active.update(db).await.unwrap();

    let state = 状態(db).await;
    let token = セッション(db, user.id, &state).await;

    let res = router(state)
        .oneshot(
            Request::builder()
                .uri("/assets/dioryga.css")
                .header(header::COOKIE, cookie_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::OK,
        "パスワード変更画面のCSSが誘導で弾かれています"
    );
}

/// CSRFトークンが無い状態変更リクエストは拒否されること（設計書20.5）。
async fn csrfトークンなしの状態変更は拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "csrf@example.com", false).await;
    let state = 状態(db).await;
    let token = セッション(db, user.id, &state).await;

    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header(header::COOKIE, cookie_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

/// 正しいCSRFトークンがあれば通ること。
async fn 正しいcsrfトークンなら通る(db: &DatabaseConnection) {
    let user = 利用者(db, "csrf-ok@example.com", false).await;
    let state = 状態(db).await;
    let token = セッション(db, user.id, &state).await;
    let csrf = dioryga::auth::csrf::derive(&token);

    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header(header::COOKIE, cookie_header(&token))
                .header(dioryga::auth::csrf::HEADER_NAME, csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/login");
}

/// **素のHTMLフォームのhidden fieldでも通ること**（設計書20.5）。
///
/// ヘッダーだけを見ていると、JavaScriptを前提にしない画面がすべて403になる。
async fn フォーム本文のcsrfトークンでも通る(db: &DatabaseConnection) {
    let user = 利用者(db, "csrf-form@example.com", false).await;
    let state = 状態(db).await;
    let token = セッション(db, user.id, &state).await;
    let csrf = dioryga::auth::csrf::derive(&token);

    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header(header::COOKIE, cookie_header(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "{}={csrf}",
                    dioryga::auth::csrf::FIELD_NAME
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::SEE_OTHER);
}

/// 本文のトークンが誤っていれば拒否されること。
async fn フォーム本文の誤ったcsrfトークンは拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "csrf-form-ng@example.com", false).await;
    let state = 状態(db).await;
    let token = セッション(db, user.id, &state).await;

    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header(header::COOKIE, cookie_header(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "{}=deadbeef",
                    dioryga::auth::csrf::FIELD_NAME
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

/// 本文を読んだあともハンドラが同じ本文を受け取れること。
///
/// 検証のために読み切った本文を差し戻さないと、フォームの項目が
/// すべて空でハンドラに届く。**403にはならないため気付きにくい。**
async fn csrf検証後もフォームの内容がハンドラへ届く(db: &DatabaseConnection) {
    let user = 利用者(db, "csrf-body@example.com", false).await;
    let mut active: app_user::ActiveModel = user.clone().into();
    active.must_change_password = Set(true);
    let user = active.update(db).await.unwrap();

    let state = 状態(db).await;
    let token = セッション(db, user.id, &state).await;
    let csrf = dioryga::auth::csrf::derive(&token);

    // 新旧が一致しないフォームを送る。本文が届いていれば「一致しません」が返る
    let body = format!(
        "{}={csrf}&current_password=x&new_password=aaaaaaaaaaaa&confirm_password=bbbbbbbbbbbb",
        dioryga::auth::csrf::FIELD_NAME
    );

    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/account/password")
                .header(header::COOKIE, cookie_header(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&bytes).contains("一致しません"),
        "フォームの内容がハンドラへ届いていません"
    );
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn cookie_header(token: &str) -> String {
    format!("{}={token}", session::COOKIE_NAME)
}

async fn セッション(db: &DatabaseConnection, user_id: i32, state: &AppState) -> String {
    let (_, token) = session::create(
        db,
        user_id,
        "127.0.0.1",
        "test",
        &state.config.session,
        Utc::now(),
    )
    .await
    .unwrap();
    token.as_str().to_owned()
}

async fn 利用者(db: &DatabaseConnection, email: &str, system_admin: bool) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証用".to_owned()),
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        password_hash: Set("dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(system_admin),
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
        全検証!(@one $用意, $属性, system_adminはロールを持っていても拒否される);
        全検証!(@one $用意, $属性, 兼務しているロールのいずれかで通る);
        全検証!(@one $用意, $属性, 別プロジェクトのロールでは通らない);
        全検証!(@one $用意, $属性, 無効化された利用者は拒否される);
        全検証!(@one $用意, $属性, カタログ編集はoperator以上でsystem_adminは不可);
        全検証!(@one $用意, $属性, 未認証は誘導される);
        全検証!(@one $用意, $属性, ログイン画面は認証不要);
        全検証!(@one $用意, $属性, system_adminはprojectsへアクセスできない);
        全検証!(@one $用意, $属性, 一般利用者はprojectsへアクセスできる);
        全検証!(@one $用意, $属性, 失効したセッションは拒否される);
        全検証!(@one $用意, $属性, パスワード変更が必要なら誘導される);
        全検証!(@one $用意, $属性, パスワード変更の強制中でも静的アセットは通る);
        全検証!(@one $用意, $属性, csrfトークンなしの状態変更は拒否される);
        全検証!(@one $用意, $属性, 正しいcsrfトークンなら通る);
        全検証!(@one $用意, $属性, フォーム本文のcsrfトークンでも通る);
        全検証!(@one $用意, $属性, フォーム本文の誤ったcsrfトークンは拒否される);
        全検証!(@one $用意, $属性, csrf検証後もフォームの内容がハンドラへ届く);
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

//! System Admin領域のユーザー管理の結合テスト（設計書16.1のA領域、20.7、20.11）。

mod support;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::app_user;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 領域の境界
// ---------------------------------------------------------------------------

/// System Adminはユーザー管理画面を開けること。
async fn system_adminは一覧を開ける(db: &DatabaseConnection) {
    let admin = 利用者(db, "list-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (status, body) = 取得(状態, "/admin/users", &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ユーザー管理"));
    assert!(body.contains("list-admin@example.com"));
}

/// **一般の利用者はSystem Admin領域へ入れないこと。**
///
/// `/projects` の逆向きのガード。片方だけでは領域が分かれない。
async fn 一般利用者はadmin領域へ入れない(db: &DatabaseConnection) {
    let user = 利用者(db, "notadmin@example.com", false).await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, _) = 取得(状態, "/admin/users", &token).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "一般利用者がSystem Admin領域に入れてしまいました"
    );
}

/// ログイン後の入口が利用者によって分かれること。
async fn ホームは役割ごとに行き先が違う(db: &DatabaseConnection) {
    let admin = 利用者(db, "home-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;
    let res = 応答(状態, "/", &token).await;
    assert_eq!(res.headers().get("location").unwrap(), "/admin/users");

    let user = 利用者(db, "home-user@example.com", false).await;
    let (状態, token) = 認証済み(db, &user).await;
    let res = 応答(状態, "/", &token).await;
    assert_eq!(res.headers().get("location").unwrap(), "/projects");
}

// ---------------------------------------------------------------------------
// 登録
// ---------------------------------------------------------------------------

/// **素のHTMLフォーム（hidden fieldのCSRFトークン）で登録できること。**
///
/// htmxを導入していない段階では、ヘッダーにトークンを載せる手段がない。
/// 本文から読めないと、すべてのフォームが403になる。
async fn フォームから利用者を登録できる(db: &DatabaseConnection) {
    let admin = 利用者(db, "create-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let res = 送信(
        状態,
        "/admin/users",
        &token,
        &[("name", "新入 太郎"), ("email", "newcomer@example.com")],
    )
    .await;

    assert_eq!(res.0, StatusCode::OK, "フォーム送信が拒否されました");

    let created = app_user::Entity::find()
        .filter(app_user::Column::Email.eq("newcomer@example.com"))
        .one(db)
        .await
        .unwrap()
        .expect("利用者が作られていません");

    assert_eq!(created.name, "新入 太郎");
    assert!(!created.is_system_admin, "既定でSystem Adminになっています");
    assert!(
        created.must_change_password,
        "本人による変更が強制されていません"
    );
    // 一時パスワードは平文で保存しない（設計書20.7）
    assert!(created.password_hash.starts_with("$argon2id$"));
}

/// 一時パスワードが応答に一度だけ現れ、DBには残らないこと（設計書20.7）。
async fn 一時パスワードは応答にだけ現れる(db: &DatabaseConnection) {
    let admin = 利用者(db, "temp-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (_, body) = 送信(
        状態,
        "/admin/users",
        &token,
        &[("name", "一時"), ("email", "temp@example.com")],
    )
    .await;

    let temporary = 一時パスワードを取り出す(&body);
    assert!(temporary.chars().count() >= 12, "短すぎます: {temporary}");

    // 再表示されないこと。平文を持たないので、一覧を開き直しても出せない
    let admin2 = app_user::Entity::find_by_id(admin.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let (状態, token) = 認証済み(db, &admin2).await;
    let (_, 再訪) = 取得(状態, "/admin/users", &token).await;
    assert!(
        !再訪.contains(&temporary),
        "一時パスワードが再表示されています"
    );
}

/// 同じメールアドレスは登録できないこと。
async fn 重複したメールアドレスは拒否される(db: &DatabaseConnection) {
    let admin = 利用者(db, "dup-admin@example.com", true).await;
    let _ = 利用者(db, "dup@example.com", false).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (status, body) = 送信(
        状態,
        "/admin/users",
        &token,
        &[("name", "重複"), ("email", "dup@example.com")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に使われています"));

    let 件数 = app_user::Entity::find()
        .filter(app_user::Column::Email.eq("dup@example.com"))
        .count(db)
        .await
        .unwrap();
    assert_eq!(件数, 1, "重複した利用者が作られています");
}

// ---------------------------------------------------------------------------
// 検索とフィルタ（設計書16.1「一覧画面共通の方針」）
// ---------------------------------------------------------------------------

async fn 検索とフィルタが効く(db: &DatabaseConnection) {
    let admin = 利用者(db, "filter-admin@example.com", true).await;
    let _ = 利用者(db, "alice@example.com", false).await;
    let bob = 利用者(db, "bob@example.com", false).await;
    無効化(db, &bob).await;

    // 既定は「有効」のみ。無効化された利用者は隠れる
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, "/admin/users", &token).await;
    assert!(body.contains("alice@example.com"));
    assert!(
        !body.contains("bob@example.com"),
        "既定で無効な利用者が出ています"
    );

    // 状態フィルタで無効のみに切り替えられる
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, "/admin/users?status=disabled", &token).await;
    assert!(body.contains("bob@example.com"));
    assert!(!body.contains("alice@example.com"));

    // キーワードで絞り込める
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, "/admin/users?q=alice&status=all", &token).await;
    assert!(body.contains("alice@example.com"));
    assert!(!body.contains("bob@example.com"));
}

/// `%` を入力しても全件一致にならないこと。
async fn ワイルドカードは打ち消される(db: &DatabaseConnection) {
    let admin = 利用者(db, "wild-admin@example.com", true).await;
    let _ = 利用者(db, "carol@example.com", false).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (_, body) = 取得(状態, "/admin/users?q=%25&status=all", &token).await;

    assert!(
        !body.contains("carol@example.com"),
        "`%` が全件一致として扱われています"
    );
}

// ---------------------------------------------------------------------------
// 無効化（設計書20.11）
// ---------------------------------------------------------------------------

/// 無効化しても行は残り、既存セッションが失効すること。
async fn 無効化しても行は残る(db: &DatabaseConnection) {
    let admin = 利用者(db, "disable-admin@example.com", true).await;
    let target = 利用者(db, "leaving@example.com", false).await;

    // 対象の利用者が既にログインしている状態を作る
    let (状態, _) = 認証済み(db, &admin).await;
    let (_, 対象トークン) = session::create(
        db,
        target.id,
        "127.0.0.1",
        "test",
        &状態.config.session,
        Utc::now(),
    )
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(
        状態,
        &format!("/admin/users/{}/disable", target.id),
        &token,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 後 = app_user::Entity::find_by_id(target.id)
        .one(db)
        .await
        .unwrap()
        .expect("利用者が物理削除されています");
    assert!(後.disabled_at.is_some());

    // 発行済みのセッションが生きていると、無効化しても操作できてしまう
    let (状態, _) = 認証済み(db, &admin).await;
    let res = 応答(状態, "/projects", 対象トークン.as_str()).await;
    assert_eq!(
        res.status(),
        StatusCode::SEE_OTHER,
        "無効化した利用者のセッションが生きています"
    );
}

/// 自分自身は無効化できないこと。
async fn 自分自身は無効化できない(db: &DatabaseConnection) {
    let admin = 利用者(db, "self-admin@example.com", true).await;
    let _ = 利用者(db, "spare-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (status, body) = 送信(
        状態,
        &format!("/admin/users/{}/disable", admin.id),
        &token,
        &[],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("自分自身は無効化できません"));

    let 後 = app_user::Entity::find_by_id(admin.id)
        .one(db)
        .await
        .unwrap();
    assert!(後.unwrap().disabled_at.is_none());
}

/// **自分自身のSystem Admin権限は外せないこと。**
///
/// 画面を操作できるのは有効なSystem Admin本人だけなので、自分自身への降格と
/// 無効化さえ止めれば「有効なSystem Adminが0人」にはならない。
async fn 自分自身は降格できない(db: &DatabaseConnection) {
    let admin = 利用者(db, "only-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    // is_system_admin を送らない＝チェックを外した送信
    let (_, body) = 送信(
        状態,
        &format!("/admin/users/{}", admin.id),
        &token,
        &[
            ("name", "最後の管理者"),
            ("email", "only-admin@example.com"),
        ],
    )
    .await;
    assert!(body.contains("System Admin権限は外せません"));

    let 後 = app_user::Entity::find_by_id(admin.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(後.is_system_admin, "System Admin権限が外れています");
}

/// 他人の編集では権限を変えられること（降格の禁止が広すぎないことの確認）。
async fn 他人にsystem_admin権限を与えられる(db: &DatabaseConnection) {
    let admin = 利用者(db, "grant-admin@example.com", true).await;
    let target = 利用者(db, "promoted@example.com", false).await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(
        状態,
        &format!("/admin/users/{}", target.id),
        &token,
        &[
            ("name", "昇格"),
            ("email", "promoted@example.com"),
            ("is_system_admin", "on"),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    let 後 = app_user::Entity::find_by_id(target.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(後.is_system_admin);
    assert_eq!(後.name, "昇格");
}

/// 無効化した利用者を再度有効にできること。
async fn 無効化した利用者を戻せる(db: &DatabaseConnection) {
    let admin = 利用者(db, "enable-admin@example.com", true).await;
    let target = 利用者(db, "returning@example.com", false).await;
    無効化(db, &target).await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(
        状態,
        &format!("/admin/users/{}/enable", target.id),
        &token,
        &[],
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    let 後 = app_user::Entity::find_by_id(target.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(後.disabled_at.is_none());
}

// ---------------------------------------------------------------------------
// パスワードリセット（設計書20.7）
// ---------------------------------------------------------------------------

async fn パスワードリセットで一時パスワードが出る(db: &DatabaseConnection) {
    let admin = 利用者(db, "reset-admin@example.com", true).await;
    let target = 利用者(db, "forgot@example.com", false).await;
    let 変更前 = target.password_hash.clone();

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, body) = 送信(
        状態,
        &format!("/admin/users/{}/reset-password", target.id),
        &token,
        &[],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("forgot@example.com"));
    let _ = 一時パスワードを取り出す(&body);

    let 後 = app_user::Entity::find_by_id(target.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(後.password_hash, 変更前);
    assert!(
        後.must_change_password,
        "次回ログイン時の変更が強制されていません"
    );
}

// ---------------------------------------------------------------------------
// ログアウト
// ---------------------------------------------------------------------------

/// レイアウトのログアウトフォームでログアウトできること。
async fn 画面からログアウトできる(db: &DatabaseConnection) {
    let admin = 利用者(db, "logout@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    // 画面にログアウトの導線があること
    let (_, body) = 取得(状態, "/admin/users", &token).await;
    assert!(body.contains("ログアウト"));
    assert!(body.contains(r#"action="/logout""#));

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(状態, "/logout", &token, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // 同じCookieでは通らなくなる
    let (状態, _) = 認証済み(db, &admin).await;
    let res = 応答(状態, "/admin/users", &token).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/login");
}

// ---------------------------------------------------------------------------
// 監査ログ（設計書24.4）
// ---------------------------------------------------------------------------

/// 画面からの変更が監査ログに残り、**パスワードハッシュが載らない**こと。
async fn 変更が監査ログに残る(db: &DatabaseConnection) {
    let admin = 利用者(db, "audit-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    送信(
        状態,
        "/admin/users",
        &token,
        &[("name", "監査"), ("email", "audited@example.com")],
    )
    .await;

    let logs = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::TableName.eq("app_user"))
        .filter(entity::audit_log::Column::Action.eq("insert"))
        .all(db)
        .await
        .unwrap();

    let 該当 = logs
        .iter()
        .find(|l| {
            l.after_json
                .as_deref()
                .is_some_and(|j| j.contains("audited@example.com"))
        })
        .expect("監査ログが記録されていません");

    assert_eq!(該当.user_id, admin.id, "操作した本人が記録されていません");
    assert!(
        !該当.after_json.as_deref().unwrap().contains("argon2id"),
        "パスワードハッシュが監査ログに載っています"
    );
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
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

fn cookie_header(token: &str) -> String {
    format!("{}={token}", session::COOKIE_NAME)
}

async fn 応答(state: AppState, uri: &str, token: &str) -> Response<Body> {
    router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie_header(token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn 取得(state: AppState, uri: &str, token: &str) -> (StatusCode, String) {
    分解(応答(state, uri, token).await).await
}

/// 素のHTMLフォームと同じ形でPOSTする。
///
/// **CSRFトークンはhidden fieldとして本文に載せる。**画面が実際に送る形と
/// 同じにしないと、テストが通っても画面は動かない。
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

/// 応答に埋め込まれた一時パスワードを取り出す。
fn 一時パスワードを取り出す(body: &str) -> String {
    let 開始 = body
        .find(r#"<p class="issued-value">"#)
        .expect("一時パスワードが表示されていません")
        + r#"<p class="issued-value">"#.len();
    let 終了 = 開始 + body[開始..].find("</p>").unwrap();
    body[開始..終了].trim().to_owned()
}

async fn 利用者(db: &DatabaseConnection, email: &str, system_admin: bool) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(format!("検証 {email}")),
        email: Set(email.to_owned()),
        password_hash: Set("$argon2id$dummy".to_owned()),
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

async fn 無効化(db: &DatabaseConnection, user: &app_user::Model) {
    let mut active: app_user::ActiveModel = user.clone().into();
    active.disabled_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, system_adminは一覧を開ける);
        全検証!(@one $用意, $属性, 一般利用者はadmin領域へ入れない);
        全検証!(@one $用意, $属性, ホームは役割ごとに行き先が違う);
        全検証!(@one $用意, $属性, フォームから利用者を登録できる);
        全検証!(@one $用意, $属性, 一時パスワードは応答にだけ現れる);
        全検証!(@one $用意, $属性, 重複したメールアドレスは拒否される);
        全検証!(@one $用意, $属性, 検索とフィルタが効く);
        全検証!(@one $用意, $属性, ワイルドカードは打ち消される);
        全検証!(@one $用意, $属性, 無効化しても行は残る);
        全検証!(@one $用意, $属性, 自分自身は無効化できない);
        全検証!(@one $用意, $属性, 自分自身は降格できない);
        全検証!(@one $用意, $属性, 他人にsystem_admin権限を与えられる);
        全検証!(@one $用意, $属性, 無効化した利用者を戻せる);
        全検証!(@one $用意, $属性, パスワードリセットで一時パスワードが出る);
        全検証!(@one $用意, $属性, 画面からログアウトできる);
        全検証!(@one $用意, $属性, 変更が監査ログに残る);
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

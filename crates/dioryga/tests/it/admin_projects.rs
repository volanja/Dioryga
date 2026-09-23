//! System Admin領域のプロジェクト管理の結合テスト（設計書16.1のA領域、5章）。

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, project, project_member};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 登録
// ---------------------------------------------------------------------------

/// プロジェクトを作成でき、`uid` が採番されること（設計書5.3）。
async fn プロジェクトを作成できる(db: &DatabaseConnection) {
    let admin = 利用者(db, "proj-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (status, _) = 送信(
        状態,
        "/admin/projects",
        &token,
        &[
            ("name", "2026年度 基盤更改"),
            ("code", "INF-2026"),
            ("description", "検証用"),
            ("currency", "JPY"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let created = project::Entity::find()
        .filter(project::Column::Code.eq("INF-2026"))
        .one(db)
        .await
        .unwrap()
        .expect("プロジェクトが作られていません");

    assert_eq!(created.name, "2026年度 基盤更改");
    assert_eq!(created.currency, "JPY");
    // 名前を変えても参照が切れないよう、Dioryga側で採番する（5.3）
    assert_eq!(
        created.uid.len(),
        36,
        "uidが採番されていません: {}",
        created.uid
    );
    // 日付と進行状態は持たない（5.1）
    assert!(created.archived_at.is_none());
    assert!(created.closure_reason.is_none());
}

/// **コード未設定はNULLで保存されること。**
///
/// 空文字にすると、UNIQUE制約により「コード未設定のプロジェクト」が
/// 2件目から作れなくなる。
async fn コード未設定は複数作れる(db: &DatabaseConnection) {
    let admin = 利用者(db, "nocode-admin@example.com", true).await;

    for name in ["コード無しA", "コード無しB"] {
        let (状態, token) = 認証済み(db, &admin).await;
        let (status, body) = 送信(
            状態,
            "/admin/projects",
            &token,
            &[("name", name), ("code", "")],
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "2件目で失敗しました: {body}");
    }

    let 件数 = project::Entity::find()
        .filter(project::Column::Code.is_null())
        .count(db)
        .await
        .unwrap();
    assert_eq!(件数, 2);
}

async fn 重複したコードは拒否される(db: &DatabaseConnection) {
    let admin = 利用者(db, "dupcode-admin@example.com", true).await;
    let _ = プロジェクト(db, "既存", Some("DUP-1")).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (status, body) = 送信(
        状態,
        "/admin/projects",
        &token,
        &[("name", "重複"), ("code", "DUP-1")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に使われています"));
    assert_eq!(
        project::Entity::find().count(db).await.unwrap(),
        1,
        "重複したプロジェクトが作られています"
    );
}

/// 未知の通貨コードは既定へ倒すこと（設計書24.2.1）。
///
/// 金額の小数点以下の桁数は通貨から決まるため、未知の値が入ると解釈できない。
async fn 未知の通貨は既定になる(db: &DatabaseConnection) {
    let admin = 利用者(db, "cur-admin@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    送信(
        状態,
        "/admin/projects",
        &token,
        &[("name", "通貨検証"), ("currency", "XYZ")],
    )
    .await;

    let created = project::Entity::find()
        .filter(project::Column::Name.eq("通貨検証"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created.currency, "JPY");
}

// ---------------------------------------------------------------------------
// 一覧
// ---------------------------------------------------------------------------

async fn 検索とフィルタが効く(db: &DatabaseConnection) {
    let admin = 利用者(db, "filter-proj@example.com", true).await;
    let _ = プロジェクト(db, "現行案件", Some("CUR-1")).await;
    let 済 = プロジェクト(db, "過年度案件", Some("OLD-1")).await;
    アーカイブ(db, &済, "Completed").await;

    // 既定は進行中のみ
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, "/admin/projects", &token).await;
    assert!(body.contains("現行案件"));
    assert!(
        !body.contains("過年度案件"),
        "既定でアーカイブ済みが出ています"
    );

    // アーカイブ済みだけに切り替えられる
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, "/admin/projects?status=archived", &token).await;
    assert!(body.contains("過年度案件"));
    assert!(!body.contains("現行案件"));

    // コードでも引ける
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, "/admin/projects?q=OLD&status=all", &token).await;
    assert!(body.contains("過年度案件"));
    assert!(!body.contains("現行案件"));
}

/// 一覧に正副の管理者名が出ること。
async fn 一覧に管理者が表示される(db: &DatabaseConnection) {
    let admin = 利用者(db, "show-admin@example.com", true).await;
    let 正 = 利用者(db, "primary@example.com", false).await;
    let p = プロジェクト(db, "管理者表示", None).await;
    メンバー(db, 正.id, p.id, "Primary").await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, "/admin/projects", &token).await;

    assert!(body.contains(&正.name));
    // 副が居なければ未割当と出る
    assert!(body.contains("未割当"));
}

// ---------------------------------------------------------------------------
// アーカイブ（設計書5.2）
// ---------------------------------------------------------------------------

/// **アーカイブには理由が要る。**完了と中止を区別しないと10.4の集計が汚れる。
async fn 理由なしではアーカイブできない(db: &DatabaseConnection) {
    let admin = 利用者(db, "arch-admin@example.com", true).await;
    let p = プロジェクト(db, "理由なし", None).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (status, body) = 送信(
        状態,
        &format!("/admin/projects/{}/archive", p.id),
        &token,
        &[("closure_reason", "")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("理由を選んでください"));

    let 後 = project::Entity::find_by_id(p.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(後.archived_at.is_none());
}

async fn 理由を付けてアーカイブできる(db: &DatabaseConnection) {
    let admin = 利用者(db, "arch-ok@example.com", true).await;
    let p = プロジェクト(db, "中止案件", None).await;
    let (状態, token) = 認証済み(db, &admin).await;

    let (status, _) = 送信(
        状態,
        &format!("/admin/projects/{}/archive", p.id),
        &token,
        &[("closure_reason", "Cancelled")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 後 = project::Entity::find_by_id(p.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(後.archived_at.is_some());
    assert_eq!(後.closure_reason.as_deref(), Some("Cancelled"));
}

/// **元に戻すと理由も消えること。**
///
/// 残すと「進行中なのに完了理由がある」という読み取れない状態になる。
async fn 元に戻すと理由も消える(db: &DatabaseConnection) {
    let admin = 利用者(db, "unarch-admin@example.com", true).await;
    let p = プロジェクト(db, "復帰案件", None).await;
    アーカイブ(db, &p, "Completed").await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(
        状態,
        &format!("/admin/projects/{}/unarchive", p.id),
        &token,
        &[],
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    let 後 = project::Entity::find_by_id(p.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(後.archived_at.is_none());
    assert!(
        後.closure_reason.is_none(),
        "終了理由が残っています: {:?}",
        後.closure_reason
    );
}

// ---------------------------------------------------------------------------
// Administratorの割り当て（設計書5章、A-5）
// ---------------------------------------------------------------------------

async fn 正副の管理者を割り当てられる(db: &DatabaseConnection) {
    let admin = 利用者(db, "assign-admin@example.com", true).await;
    let 正 = 利用者(db, "a-primary@example.com", false).await;
    let 副 = 利用者(db, "a-secondary@example.com", false).await;
    let p = プロジェクト(db, "割り当て", None).await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(
        状態,
        &format!("/admin/projects/{}/members", p.id),
        &token,
        &[
            ("primary", &正.id.to_string()),
            ("secondary", &副.id.to_string()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let rows = 管理者行(db, p.id).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rank(&rows, 正.id), Some("Primary".to_owned()));
    assert_eq!(rank(&rows, 副.id), Some("Secondary".to_owned()));
}

/// **正副それぞれ最大1名であること**（A-5）。
///
/// 割り当て直しても行が増えず、古い割り当てが残らない。
async fn 割り当て直しても正副は各1名(db: &DatabaseConnection) {
    let admin = 利用者(db, "reassign-admin@example.com", true).await;
    let 旧 = 利用者(db, "old-primary@example.com", false).await;
    let 新 = 利用者(db, "new-primary@example.com", false).await;
    let p = プロジェクト(db, "割り当て直し", None).await;

    for user in [&旧, &新] {
        let (状態, token) = 認証済み(db, &admin).await;
        送信(
            状態,
            &format!("/admin/projects/{}/members", p.id),
            &token,
            &[("primary", &user.id.to_string()), ("secondary", "")],
        )
        .await;
    }

    let rows = 管理者行(db, p.id).await;
    assert_eq!(rows.len(), 1, "古い割り当てが残っています");
    assert_eq!(rows[0].user_id, 新.id);
}

/// **兼務している他のロールを巻き添えにしないこと**（設計書5章）。
async fn 割り当て直しても他のロールは残る(db: &DatabaseConnection) {
    let admin = 利用者(db, "keeprole-admin@example.com", true).await;
    let user = 利用者(db, "multi-role@example.com", false).await;
    let p = プロジェクト(db, "兼務", None).await;
    メンバー(db, user.id, p.id, "Operator").await;

    let (状態, token) = 認証済み(db, &admin).await;
    送信(
        状態,
        &format!("/admin/projects/{}/members", p.id),
        &token,
        &[("primary", &user.id.to_string())],
    )
    .await;

    let operator = project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(p.id))
        .filter(project_member::Column::Role.eq("Operator"))
        .one(db)
        .await
        .unwrap();
    assert!(operator.is_some(), "Operatorの行が消えています");
}

/// **System Adminは割り当てられないこと**（設計書3章）。
///
/// 割り当てられてもプロジェクトデータへアクセスできず、画面と挙動が食い違う。
async fn system_adminは割り当てられない(db: &DatabaseConnection) {
    let admin = 利用者(db, "sys-assign@example.com", true).await;
    let 別のsysadmin = 利用者(db, "another-sys@example.com", true).await;
    let p = プロジェクト(db, "System Admin検証", None).await;

    // 画面の候補に出ないこと
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, &format!("/admin/projects/{}/members", p.id), &token).await;
    assert!(
        !body.contains("another-sys@example.com"),
        "候補に出ています"
    );

    // 選択肢を絞るだけでなく、送信内容も拒否すること
    let (状態, token) = 認証済み(db, &admin).await;
    let (status, body) = 送信(
        状態,
        &format!("/admin/projects/{}/members", p.id),
        &token,
        &[("primary", &別のsysadmin.id.to_string())],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("割り当てできない"));
    assert!(管理者行(db, p.id).await.is_empty());
}

/// **候補が0件のとき、理由と次の一手を出すこと**（#90）。
///
/// 初回セットアップ直後はSystem Adminしか居ないため、候補が1人も出ない。
/// **画面は「正管理者のみでも登録できます」と書いており、空のプルダウンは
/// 不具合に見える。**空なのが実態なのか設定漏れなのかを判別できる必要がある。
async fn 候補がゼロなら理由を出す(db: &DatabaseConnection) {
    let admin = 利用者(db, "only-sysadmin@example.com", true).await;
    let p = プロジェクト(db, "候補ゼロ検証", None).await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, body) = 取得(状態, &format!("/admin/projects/{}/members", p.id), &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("割り当てられる利用者がいません"),
        "理由が出ていない"
    );
    // **次の一手を示す。**サイドナビまで戻らせない
    assert!(body.contains("/admin/users"), "ユーザー管理への導線が無い");
    // 選べない以上、保存欄は出さない
    assert!(
        !body.contains("name=\"primary\""),
        "空のプルダウンが出ている"
    );
}

/// 候補が1件でもあれば、従来どおり選択欄を出すこと（#90）。
async fn 候補があれば選択欄を出す(db: &DatabaseConnection) {
    let admin = 利用者(db, "has-candidate@example.com", true).await;
    利用者(db, "member@example.com", false).await;
    let p = プロジェクト(db, "候補あり検証", None).await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 取得(状態, &format!("/admin/projects/{}/members", p.id), &token).await;

    assert!(body.contains("name=\"primary\""));
    assert!(body.contains("member@example.com"));
    assert!(!body.contains("割り当てられる利用者がいません"));
}

/// 無効化された利用者は割り当てられないこと（設計書20.11）。
async fn 無効化された利用者は割り当てられない(db: &DatabaseConnection) {
    let admin = 利用者(db, "disabled-assign@example.com", true).await;
    let 退職者 = 利用者(db, "left@example.com", false).await;
    let mut active: app_user::ActiveModel = 退職者.clone().into();
    active.disabled_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();

    let p = プロジェクト(db, "無効化検証", None).await;
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 送信(
        状態,
        &format!("/admin/projects/{}/members", p.id),
        &token,
        &[("primary", &退職者.id.to_string())],
    )
    .await;

    assert!(body.contains("割り当てできない"));
    assert!(管理者行(db, p.id).await.is_empty());
}

/// 同一人物を正副に置けないこと。冗長化にならず、正副を分けた意味が失われる。
async fn 同じ利用者を正副にできない(db: &DatabaseConnection) {
    let admin = 利用者(db, "same-admin@example.com", true).await;
    let user = 利用者(db, "same-person@example.com", false).await;
    let p = プロジェクト(db, "同一人物", None).await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 送信(
        状態,
        &format!("/admin/projects/{}/members", p.id),
        &token,
        &[
            ("primary", &user.id.to_string()),
            ("secondary", &user.id.to_string()),
        ],
    )
    .await;

    assert!(body.contains("同じ利用者は指定できません"));
    assert!(管理者行(db, p.id).await.is_empty());
}

/// 副だけの割り当てはできないこと（設計書5章：Primaryのみでも可、その逆はない）。
async fn 副だけは割り当てられない(db: &DatabaseConnection) {
    let admin = 利用者(db, "only-sec-admin@example.com", true).await;
    let user = 利用者(db, "only-secondary@example.com", false).await;
    let p = プロジェクト(db, "副のみ", None).await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (_, body) = 送信(
        状態,
        &format!("/admin/projects/{}/members", p.id),
        &token,
        &[("primary", ""), ("secondary", &user.id.to_string())],
    )
    .await;

    assert!(body.contains("副管理者だけ"));
    assert!(管理者行(db, p.id).await.is_empty());
}

// ---------------------------------------------------------------------------
// 領域の境界
// ---------------------------------------------------------------------------

/// 一般の利用者はプロジェクト管理画面へ入れないこと。
async fn 一般利用者は入れない(db: &DatabaseConnection) {
    let user = 利用者(db, "plain-user@example.com", false).await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, _) = 取得(状態, "/admin/projects", &token).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// 変更が監査ログに残ること（設計書24.4）。
async fn 変更が監査ログに残る(db: &DatabaseConnection) {
    let admin = 利用者(db, "audit-proj@example.com", true).await;
    let (状態, token) = 認証済み(db, &admin).await;

    送信(
        状態,
        "/admin/projects",
        &token,
        &[("name", "監査対象"), ("code", "AUD-1")],
    )
    .await;

    let logs = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::TableName.eq("project"))
        .all(db)
        .await
        .unwrap();

    let 該当 = logs
        .iter()
        .find(|l| {
            l.after_json
                .as_deref()
                .is_some_and(|j| j.contains("監査対象"))
        })
        .expect("監査ログが記録されていません");

    assert_eq!(該当.user_id, admin.id);
    assert_eq!(該当.action, "insert");
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

/// 素のHTMLフォームと同じ形でPOSTする（CSRFトークンはhidden field）。
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

async fn 利用者(db: &DatabaseConnection, email: &str, system_admin: bool) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(format!("検証 {email}")),
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
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

async fn プロジェクト(
    db: &DatabaseConnection,
    name: &str,
    code: Option<&str>,
) -> project::Model {
    project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(code.map(str::to_owned)),
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

async fn アーカイブ(db: &DatabaseConnection, p: &project::Model, reason: &str) {
    let mut active: project::ActiveModel = p.clone().into();
    active.archived_at = Set(Some(Utc::now()));
    active.closure_reason = Set(Some(reason.to_owned()));
    active.update(db).await.unwrap();
}

/// `rank` が `Some` なら Administrator、`None` ならその他のロールとして作る。
async fn メンバー(db: &DatabaseConnection, user_id: i32, project_id: i32, rank: &str) {
    let (role, admin_rank) = match rank {
        "Primary" | "Secondary" => ("Administrator", Some(rank.to_owned())),
        other => (other, None),
    };

    project_member::ActiveModel {
        user_id: Set(user_id),
        project_id: Set(project_id),
        role: Set(role.to_owned()),
        admin_rank: Set(admin_rank),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 管理者行(db: &DatabaseConnection, project_id: i32) -> Vec<project_member::Model> {
    project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .filter(project_member::Column::Role.eq("Administrator"))
        .all(db)
        .await
        .unwrap()
}

fn rank(rows: &[project_member::Model], user_id: i32) -> Option<String> {
    rows.iter()
        .find(|m| m.user_id == user_id)
        .and_then(|m| m.admin_rank.clone())
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, プロジェクトを作成できる);
        全検証!(@one $用意, $属性, コード未設定は複数作れる);
        全検証!(@one $用意, $属性, 重複したコードは拒否される);
        全検証!(@one $用意, $属性, 未知の通貨は既定になる);
        全検証!(@one $用意, $属性, 検索とフィルタが効く);
        全検証!(@one $用意, $属性, 一覧に管理者が表示される);
        全検証!(@one $用意, $属性, 理由なしではアーカイブできない);
        全検証!(@one $用意, $属性, 理由を付けてアーカイブできる);
        全検証!(@one $用意, $属性, 元に戻すと理由も消える);
        全検証!(@one $用意, $属性, 正副の管理者を割り当てられる);
        全検証!(@one $用意, $属性, 割り当て直しても正副は各1名);
        全検証!(@one $用意, $属性, 割り当て直しても他のロールは残る);
        全検証!(@one $用意, $属性, system_adminは割り当てられない);
        全検証!(@one $用意, $属性, 候補がゼロなら理由を出す);
        全検証!(@one $用意, $属性, 候補があれば選択欄を出す);
        全検証!(@one $用意, $属性, 無効化された利用者は割り当てられない);
        全検証!(@one $用意, $属性, 同じ利用者を正副にできない);
        全検証!(@one $用意, $属性, 副だけは割り当てられない);
        全検証!(@one $用意, $属性, 一般利用者は入れない);
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

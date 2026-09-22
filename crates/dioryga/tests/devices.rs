//! プロジェクト領域の機器画面の結合テスト（設計書16.1のB領域、3章、A-6）。

mod support;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::{Duration, Utc};
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, device, device_assignment, project, project_member};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// クロスプロジェクト可視性（A-6）
// ---------------------------------------------------------------------------

/// **過去に所属した機器も閲覧できること**（A-6、設計書3章）。
///
/// 現在の割当だけで絞ると、移設された機器の履歴が誰からも見えなくなる。
/// 既定の一覧では現役に絞るが、範囲を広げれば出る。
async fn 過去に所属した機器も閲覧できる(db: &DatabaseConnection) {
    let user = 利用者(db, "a6@example.com").await;
    let 旧 = プロジェクト(db, "移管元").await;
    let 新 = プロジェクト(db, "移管先").await;
    メンバー(db, user.id, 旧.id, "Viewer").await;

    let d = 機器(db, "moved-01").await;
    // 旧プロジェクトに居た → 新プロジェクトへ移した
    let 昔 = 割当(db, d.id, 旧.id, Utc::now() - Duration::days(30)).await;
    閉じる(db, 昔).await;
    割当(db, d.id, 新.id, Utc::now()).await;

    // 既定（現在このプロジェクトにあるもの）には出ない
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, &format!("/projects/{}/devices", 旧.id), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("moved-01"), "既定で移管済みが出ています");

    // 範囲を広げると出る。**これがA-6の要点**
    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/devices?scope=all", 旧.id),
        &token,
    )
    .await;
    assert!(
        body.contains("moved-01"),
        "過去に所属した機器が見えません（A-6違反）"
    );

    // 詳細も開ける。所在の履歴が両方出る
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", 旧.id, d.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("移管元"));
    assert!(body.contains("移管先"), "移設後の所在が見えません");
}

/// **所属したことのないプロジェクトからは見えないこと。**
///
/// URLを直接叩かれても通さない。A-6は「一度でも所属したか」で判定する。
async fn 無関係なプロジェクトからは見えない(db: &DatabaseConnection) {
    let user = 利用者(db, "unrelated@example.com").await;
    let 自分の = プロジェクト(db, "自分のプロジェクト").await;
    let 他人の = プロジェクト(db, "他人のプロジェクト").await;
    メンバー(db, user.id, 自分の.id, "Administrator").await;

    let d = 機器(db, "secret-01").await;
    割当(db, d.id, 他人の.id, Utc::now()).await;

    // 一覧に出ない
    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/devices?scope=all", 自分の.id),
        &token,
    )
    .await;
    assert!(!body.contains("secret-01"));

    // IDを直接指定しても見えない
    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", 自分の.id, d.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 所属していないプロジェクトの画面自体に入れないこと。
async fn 非メンバーは入れない(db: &DatabaseConnection) {
    let user = 利用者(db, "nonmember@example.com").await;
    let p = プロジェクト(db, "よそのプロジェクト").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// ロールによる編集の可否
// ---------------------------------------------------------------------------

/// **ViewerとApproverは編集できないこと。**
///
/// Approverは承認する立場であり、自分で変更を入れられると承認の意味が薄れる。
async fn 閲覧のみのロールは登録できない(db: &DatabaseConnection) {
    for (email, role) in [
        ("viewer@example.com", "Viewer"),
        ("approver@example.com", "Approver"),
    ] {
        let user = 利用者(db, email).await;
        let p = プロジェクト(db, &format!("{role}検証")).await;
        メンバー(db, user.id, p.id, role).await;

        // 登録画面に入れない
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 取得(状態, &format!("/projects/{}/devices/new", p.id), &token).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{role} が登録画面に入れました"
        );

        // 送信しても拒否される。画面のボタンを隠すだけでは足りない
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 送信(
            状態,
            &format!("/projects/{}/devices", p.id),
            &token,
            &[("hostname", "sneaky-01")],
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{role} が登録できました");
    }

    assert_eq!(
        device::Entity::find().all(db).await.unwrap().len(),
        0,
        "機器が作られています"
    );
}

async fn operatorは登録できる(db: &DatabaseConnection) {
    let user = 利用者(db, "operator@example.com").await;
    let p = プロジェクト(db, "登録検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "srv-new"),
            ("device_type", "Physical"),
            ("status", "provisioning"),
            ("power_watt", "350"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let created = device::Entity::find()
        .filter(device::Column::Hostname.eq("srv-new"))
        .one(db)
        .await
        .unwrap()
        .expect("機器が作られていません");

    // 採番待ちでも登録できる（設計書23.2）
    assert!(created.asset_number.is_none());
    assert_eq!(created.uid.len(), 36);

    // **所在の割当が同時に作られること。**書かないとどの一覧にも出てこない
    let 割当 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(created.id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .expect("所在の割当がありません");
    assert_eq!(割当.location_type, "Project");
    assert_eq!(割当.location_id, Some(p.id));
}

/// **不正な入力は拒否すること**（設計書8.6、Q-21）。
///
/// 選択肢はサーバが描画しているため、語彙外の値が届くのは改竄か
/// クライアントの不具合しかありえない。**黙って別の値を保存すると、
/// どちらの場合も気付けない。**
async fn 不正な入力は拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "invalid@example.com").await;
    let p = プロジェクト(db, "検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let 検証項目: &[(&str, &str, &str)] = &[
        ("device_type", "でたらめ", "種別の値が不正です"),
        ("status", "でたらめ", "状態の値が不正です"),
        ("device_category", "でたらめ", "種別分類の値が不正です"),
        // 数値として読めない
        ("configuration_id", "abc", "構成の指定が不正です"),
        // **存在しないID。**確かめないと外部キー違反で500になる
        ("configuration_id", "9999", "構成の指定が不正です"),
        ("power_watt", "abc", "消費電力は0以上"),
        ("power_watt", "-1", "消費電力は0以上"),
    ];

    for (key, value, 期待) in 検証項目 {
        // **同じキーを2度送らない。**重複するとフォームの解釈自体が失敗し
        // （422）、検証を通ったのか弾かれたのかが判別できなくなる
        let mut fields: Vec<(&str, &str)> = vec![
            ("hostname", "invalid-01"),
            ("device_type", "Physical"),
            ("status", "provisioning"),
        ];
        match fields.iter_mut().find(|(k, _)| k == key) {
            Some(項目) => 項目.1 = value,
            None => fields.push((key, value)),
        }

        let (状態, token) = 認証済み(db, &user).await;
        let (status, body) = 送信(
            状態,
            &format!("/projects/{}/devices", p.id),
            &token,
            &fields,
        )
        .await;

        assert_eq!(
            status,
            StatusCode::OK,
            "{key}={value} が拒否されませんでした"
        );
        assert!(body.contains(期待), "{key}={value} のエラーが出ていません");
    }

    assert_eq!(
        device::Entity::find().all(db).await.unwrap().len(),
        0,
        "不正な入力で機器が作られています"
    );
}

/// 空欄が許される項目と、許されない項目を取り違えないこと。
///
/// 構成・種別分類・シリアル番号・資産番号は空でよい。仮想機器は構成も
/// シリアル番号も持たず（設計書6.2）、資産番号は採番待ちがありうる（23.2）。
async fn 空欄が許される項目は通ること(db: &DatabaseConnection) {
    let user = 利用者(db, "blank@example.com").await;
    let p = プロジェクト(db, "空欄検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "vm-blank"),
            ("device_type", "Virtual"),
            ("status", "running"),
            ("configuration_id", ""),
            ("device_category", ""),
            ("serial_number", ""),
            ("asset_number", ""),
            ("power_watt", ""),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER, "拒否されました: {body}");
    let created = device::Entity::find()
        .filter(device::Column::Hostname.eq("vm-blank"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created.power_watt, 0);
    assert!(created.configuration_id.is_none());
}

// ---------------------------------------------------------------------------
// 表示
// ---------------------------------------------------------------------------

/// 予約中の機器が区別表示されること（設計書11.6）。
async fn 予約中の機器は区別表示される(db: &DatabaseConnection) {
    let user = 利用者(db, "plan@example.com").await;
    let p = プロジェクト(db, "予約検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let d = 機器(db, "planned-01").await;
    let mut active: device::ActiveModel = d.clone().into();
    active.status = Set("planned".to_owned());
    active.update(db).await.unwrap();
    割当(db, d.id, p.id, Utc::now()).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;

    assert!(body.contains("planned-01"));
    assert!(body.contains("予約中"), "予約中の区別表示がありません");
}

/// **統合された機器は一覧に出ないこと**（設計書23.9）。
///
/// 重複として畳まれた側であり、出すと同じ機器が2行に見える。
/// 詳細を直接開いた場合は統合先へ誘導する。
async fn 統合された機器は一覧に出ない(db: &DatabaseConnection) {
    let user = 利用者(db, "merged@example.com").await;
    let p = プロジェクト(db, "統合検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let 残す = 機器(db, "keep-01").await;
    let 吸収 = 機器(db, "absorbed-01").await;
    割当(db, 残す.id, p.id, Utc::now()).await;
    割当(db, 吸収.id, p.id, Utc::now()).await;

    let mut active: device::ActiveModel = 吸収.clone().into();
    active.merged_into_device_id = Set(Some(残す.id));
    active.merged_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/devices?scope=all", p.id),
        &token,
    )
    .await;
    assert!(body.contains("keep-01"));
    assert!(!body.contains("absorbed-01"), "統合された機器が出ています");

    // 詳細は開けて、統合先へ誘導される
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", p.id, 吸収.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("重複として統合されました"));
}

/// 所属するプロジェクトだけが一覧に出ること。
async fn 所属するプロジェクトだけ見える(db: &DatabaseConnection) {
    let user = 利用者(db, "list@example.com").await;
    let 自分の = プロジェクト(db, "所属している").await;
    let _ = プロジェクト(db, "所属していない").await;
    メンバー(db, user.id, 自分の.id, "Operator").await;
    メンバー(db, user.id, 自分の.id, "Approver").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, "/projects", &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("所属している"));
    assert!(!body.contains("所属していない"));
    // 兼務しているロールが両方出る（設計書5章）
    assert!(body.contains("Operator"));
    assert!(body.contains("Approver"));
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

/// **種別は日本語画面で訳して出し、保存する値は語彙のままであること**（#126）。
///
/// 略語（`VPN` 等）は訳さない。一覧・詳細・登録フォームの3画面を見る。
async fn 種別は表示だけを訳す(db: &DatabaseConnection) {
    let user = 利用者(db, "category-label@example.com").await;
    let p = プロジェクト(db, "種別表示").await;
    メンバー(db, user.id, p.id, "Operator").await;
    let d = 機器(db, "vsrv-01").await;
    割当(db, d.id, p.id, Utc::now()).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 一覧) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;
    assert!(一覧.contains("<td>サーバー</td>"), "一覧で訳されていません");

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 詳細) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", p.id, d.id),
        &token,
    )
    .await;
    assert!(詳細.contains("<dd>サーバー</dd>"), "詳細で訳されていません");

    let (状態, token) = 認証済み(db, &user).await;
    let (_, フォーム) = 取得(状態, &format!("/projects/{}/devices/new", p.id), &token).await;
    assert!(
        フォーム.contains(r#"<option value="Server" >サーバー</option>"#),
        "{フォーム}"
    );
    assert!(フォーム.contains(r#"<option value="VPN" >VPN</option>"#));
}

/// **一覧の状態にランプを添えること**（#163、設計書16.4）。
///
/// 文字だけだと、並んだ行の中で状態を見つけるのに目が要る。
async fn 一覧の状態にランプが付く(db: &DatabaseConnection) {
    let user = 利用者(db, "row-led@example.com").await;
    let p = プロジェクト(db, "ランプ検証").await;
    メンバー(db, user.id, p.id, "Viewer").await;
    let d = 機器(db, "led-01").await;
    割当(db, d.id, p.id, Utc::now()).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;
    assert!(body.contains(r#"<span class="led sm running">"#), "{body}");
    // ホスト名は等幅の列（0とOの判別が要る）
    assert!(body.contains(r#"<td class="mono">"#), "{body}");
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
        name: Set("検証".to_owned()),
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

async fn 機器(db: &DatabaseConnection, hostname: &str) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        external_id: Set(None),
        merged_into_device_id: Set(None),
        merged_at: Set(None),
        configuration_id: Set(None),
        device_type: Set("Physical".to_owned()),
        device_category: Set(Some("Server".to_owned())),
        hostname: Set(hostname.to_owned()),
        serial_number: Set(None),
        asset_number: Set(None),
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

async fn 割当(
    db: &DatabaseConnection,
    device_id: i32,
    project_id: i32,
    from: chrono::DateTime<Utc>,
) -> device_assignment::Model {
    device_assignment::ActiveModel {
        device_id: Set(device_id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(project_id)),
        work_order_id: Set(None),
        from_date: Set(from),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 閉じる(db: &DatabaseConnection, row: device_assignment::Model) {
    let mut active: device_assignment::ActiveModel = row.into();
    active.to_date = Set(Some(Utc::now()));
    active.update(db).await.unwrap();
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 過去に所属した機器も閲覧できる);
        全検証!(@one $用意, $属性, 無関係なプロジェクトからは見えない);
        全検証!(@one $用意, $属性, 非メンバーは入れない);
        全検証!(@one $用意, $属性, 閲覧のみのロールは登録できない);
        全検証!(@one $用意, $属性, operatorは登録できる);
        全検証!(@one $用意, $属性, 不正な入力は拒否される);
        全検証!(@one $用意, $属性, 空欄が許される項目は通ること);
        全検証!(@one $用意, $属性, 予約中の機器は区別表示される);
        全検証!(@one $用意, $属性, 統合された機器は一覧に出ない);
        全検証!(@one $用意, $属性, 所属するプロジェクトだけ見える);
        全検証!(@one $用意, $属性, 種別は表示だけを訳す);
        全検証!(@one $用意, $属性, 一覧の状態にランプが付く);
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

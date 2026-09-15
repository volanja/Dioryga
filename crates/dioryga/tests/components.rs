//! コンポーネント検索の結合テスト（設計書9.8）。
//!
//! **9.8が「両DB共通のSQLで書ける」と主張している以上、そのSQLが両方で実際に
//! 動くことを確かめる価値がある。**#58 のラック図でも同じ理由で生のSQLを流した。
//!
//! 加えて、この画面の存在意義そのものを確かめる。
//!
//! - **同じイメージの機器が何台あっても、その全台が引けること**（9.8）。索引は
//!   `content_hash` 単位なので、結合が間違っていると1台しか出ない
//! - 3章のクロスプロジェクト可視性が `SBOM_IMPORT.device_id` を経由すること
//! - 過去時点の検索が `superseded_at` の条件だけで行えること

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
    app_user, device, device_assignment, project, project_member, sbom_component_index,
    sbom_import, sbom_snapshot,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use std::sync::Arc;
use tower::ServiceExt;

const LOG4J: &str = "pkg:maven/org.apache.logging.log4j/log4j-core";

// ---------------------------------------------------------------------------
// 検索（設計書9.8）
// ---------------------------------------------------------------------------

/// **同じイメージの3台がすべて引けること**（設計書9.8）。
///
/// 索引は `content_hash` 単位で1組しか無い。結合が間違っていると1台しか出ない。
async fn 同じイメージの全台が引ける(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;
    let hash = スナップショット(db, "aa").await;
    索引(
        db,
        &hash,
        Some(&format!("{LOG4J}@2.14.1")),
        "log4j-core",
        "2.14.1",
    )
    .await;

    let mut ids = Vec::new();
    for i in 0..3 {
        let d = 機器(db, &format!("web-{i}"), 場.project_id).await;
        取込(db, d, &hash, 場.user.id, false).await;
        ids.push(d);
    }

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 検索(状態, &token, 場.project_id, LOG4J, "current").await;

    assert_eq!(status, StatusCode::OK);
    for i in 0..3 {
        assert!(body.contains(&format!("web-{i}")), "web-{i} が出ていない");
    }
    // 索引は1組のまま
    assert_eq!(索引数(db).await, 1);
}

/// **PURLは前方一致で引けること**（設計書9.8のSQL）。
///
/// PURLは版を含むため、版を書かずに「この製品が入っている機器」を探せる必要がある。
async fn purlは前方一致で引ける(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;

    for (suffix, version) in [("aa", "2.14.1"), ("bb", "2.17.1")] {
        let hash = スナップショット(db, suffix).await;
        索引(
            db,
            &hash,
            Some(&format!("{LOG4J}@{version}")),
            "log4j-core",
            version,
        )
        .await;
        let d = 機器(db, &format!("srv-{version}"), 場.project_id).await;
        取込(db, d, &hash, 場.user.id, false).await;
    }

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 検索(状態, &token, 場.project_id, LOG4J, "current").await;

    // 版を書かずに両方引ける
    assert!(body.contains("srv-2.14.1"));
    assert!(body.contains("srv-2.17.1"));

    // 版まで書けば1台に絞れる
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, 絞り込み) = 検索(
        状態,
        &token,
        場.project_id,
        &format!("{LOG4J}@2.14.1"),
        "current",
    )
    .await;
    assert!(絞り込み.contains("srv-2.14.1"));
    assert!(!絞り込み.contains("srv-2.17.1"));
}

/// **`purl` を持たないコンポーネントも名前で引けること**（設計書9.6、9.8）。
async fn purlが無くても名前で引ける(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;
    let hash = スナップショット(db, "cc").await;
    索引(db, &hash, None, "custom-agent", "1.2.3").await;
    let d = 機器(db, "agent-host", 場.project_id).await;
    取込(db, d, &hash, 場.user.id, false).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 検索(状態, &token, 場.project_id, "custom-agent", "current").await;
    assert!(body.contains("agent-host"));
}

/// **過去の観測は既定で出ないこと**（設計書9.8）。
///
/// 既定は現在の構成。切り替えると過去も引ける。
async fn 過去の観測は切り替えで引ける(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;
    let d = 機器(db, "patched-01", 場.project_id).await;

    // 古い観測（脆弱な版）を閉じ、新しい観測に差し替える
    let 旧 = スナップショット(db, "old").await;
    索引(
        db,
        &旧,
        Some(&format!("{LOG4J}@2.14.1")),
        "log4j-core",
        "2.14.1",
    )
    .await;
    取込(db, d, &旧, 場.user.id, true).await;

    let 新 = スナップショット(db, "new").await;
    索引(
        db,
        &新,
        Some(&format!("{LOG4J}@2.17.1")),
        "log4j-core",
        "2.17.1",
    )
    .await;
    取込(db, d, &新, 場.user.id, false).await;

    // 現在の構成には脆弱な版が無い
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, 現在) = 検索(
        状態,
        &token,
        場.project_id,
        &format!("{LOG4J}@2.14.1"),
        "current",
    )
    .await;
    assert!(
        !現在.contains("patched-01"),
        "解消済みの版が現在として出ている"
    );

    // 過去を含めれば「あの日入っていた」ことが分かる
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, 過去) = 検索(
        状態,
        &token,
        場.project_id,
        &format!("{LOG4J}@2.14.1"),
        "all",
    )
    .await;
    assert!(過去.contains("patched-01"));
    assert!(過去.contains("過去"), "過去の印が付いていない");
}

// ---------------------------------------------------------------------------
// 可視性（設計書3章、9.8）
// ---------------------------------------------------------------------------

/// **他プロジェクトの機器は出ないこと**（設計書9.8）。
///
/// スナップショットが共有されていても、閲覧できるのは自分が権限を持つDeviceの
/// 取込行を通じてのみである。**ここが漏れると、共有した瞬間に他社の機器名が
/// 見えることになる。**
async fn 他プロジェクトの機器は出ない(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;
    let 他人の = プロジェクト(db, "他人のプロジェクト").await;

    // **同じスナップショットを共有している**
    let hash = スナップショット(db, "shared").await;
    索引(
        db,
        &hash,
        Some(&format!("{LOG4J}@2.14.1")),
        "log4j-core",
        "2.14.1",
    )
    .await;

    let 自分の機器 = 機器(db, "mine-01", 場.project_id).await;
    let よその機器 = 機器(db, "theirs-01", 他人の.id).await;
    取込(db, 自分の機器, &hash, 場.user.id, false).await;
    取込(db, よその機器, &hash, 場.user.id, false).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 検索(状態, &token, 場.project_id, LOG4J, "current").await;

    assert!(body.contains("mine-01"));
    assert!(
        !body.contains("theirs-01"),
        "他プロジェクトの機器が漏れている"
    );
}

/// **A-6により、過去に所属した機器も対象に含めること**（設計書3章）。
///
/// 移設された機器に脆弱性が残っていても見えなくなっては困る。
async fn 移設された機器も対象に含む(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;
    let 移設先 = プロジェクト(db, "移設先").await;

    let hash = スナップショット(db, "moved").await;
    索引(
        db,
        &hash,
        Some(&format!("{LOG4J}@2.14.1")),
        "log4j-core",
        "2.14.1",
    )
    .await;

    let d = 機器(db, "moved-01", 場.project_id).await;
    取込(db, d, &hash, 場.user.id, false).await;

    // 移設する——元の行を閉じ、移設先の行を開く
    let 旧 = device_assignment::Entity::find()
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .find(|a| a.device_id == d)
        .unwrap();
    device_assignment::ActiveModel {
        id: Set(旧.id),
        to_date: Set(Some(Utc::now())),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
    所在(db, d, 移設先.id).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 検索(状態, &token, 場.project_id, LOG4J, "current").await;
    assert!(body.contains("moved-01"), "移設された機器が引けない");
}

/// メンバーでなければ入れないこと（設計書3章）。
async fn 部外者は入れない(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;
    let 他人の = プロジェクト(db, "他人のプロジェクト").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 検索(状態, &token, 他人の.id, LOG4J, "current").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 画面の振る舞い
// ---------------------------------------------------------------------------

/// **検索していない状態と0件を区別すること。**
async fn 未検索と零件を区別する(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, 未検索) = 取得(
        状態,
        &format!("/projects/{}/software/components", 場.project_id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(未検索.contains("検索語を入力してください"));

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, 零件) = 検索(状態, &token, 場.project_id, "存在しないもの", "current").await;
    assert!(零件.contains("該当するコンポーネントがありません"));
}

/// `%` や `_` を含む検索語がワイルドカードとして解釈されないこと。
async fn 検索語のワイルドカードを打ち消す(db: &DatabaseConnection) {
    let 場 = 舞台(db).await;
    let hash = スナップショット(db, "esc").await;
    索引(db, &hash, None, "plain-lib", "1.0").await;
    let d = 機器(db, "esc-01", 場.project_id).await;
    取込(db, d, &hash, 場.user.id, false).await;

    // `%` がそのまま渡ると全件に当たってしまう
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 検索(状態, &token, 場.project_id, "%", "current").await;
    assert!(
        !body.contains("esc-01"),
        "ワイルドカードが効いてしまっている"
    );
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project_id: i32,
}

async fn 舞台(db: &DatabaseConnection) -> 舞台情報 {
    let user = 利用者(db, "search@example.com").await;
    let p = プロジェクト(db, "基幹刷新").await;
    メンバー(db, user.id, p.id, "Operator").await;
    舞台情報 {
        user,
        project_id: p.id,
    }
}

async fn 検索(
    state: AppState,
    token: &str,
    project_id: i32,
    q: &str,
    scope: &str,
) -> (StatusCode, String) {
    let uri = format!(
        "/projects/{project_id}/software/components?q={}&scope={scope}",
        urlencode(q)
    );
    取得(state, &uri, token).await
}

fn urlencode(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn スナップショット(db: &DatabaseConnection, seed: &str) -> String {
    let hash = seed.repeat(32)[..64].to_owned();
    sbom_snapshot::ActiveModel {
        content_hash: Set(hash.clone()),
        content: Set(zstd::encode_all(&b"[]"[..], 3).unwrap()),
        component_count: Set(1),
        first_seen_at: Set(Utc::now()),
    }
    .insert(db)
    .await
    .unwrap();
    hash
}

async fn 索引(
    db: &DatabaseConnection,
    content_hash: &str,
    purl: Option<&str>,
    name: &str,
    version: &str,
) {
    sbom_component_index::ActiveModel {
        content_hash: Set(content_hash.to_owned()),
        purl: Set(purl.map(str::to_owned)),
        name: Set(name.to_owned()),
        version: Set(version.to_owned()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 索引数(db: &DatabaseConnection) -> usize {
    sbom_component_index::Entity::find()
        .all(db)
        .await
        .unwrap()
        .len()
}

async fn 取込(
    db: &DatabaseConnection,
    device_id: i32,
    content_hash: &str,
    by: i32,
    superseded: bool,
) {
    sbom_import::ActiveModel {
        device_id: Set(device_id),
        content_hash: Set(content_hash.to_owned()),
        source_format: Set("CycloneDX".to_owned()),
        work_order_id: Set(None),
        imported_by: Set(by),
        imported_at: Set(Utc::now()),
        superseded_at: Set(superseded.then(Utc::now)),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 機器(db: &DatabaseConnection, hostname: &str, project_id: i32) -> i32 {
    let d = device::ActiveModel {
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
    .unwrap();
    所在(db, d.id, project_id).await;
    d.id
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
        全検証!(@one $用意, $属性, 同じイメージの全台が引ける);
        全検証!(@one $用意, $属性, purlは前方一致で引ける);
        全検証!(@one $用意, $属性, purlが無くても名前で引ける);
        全検証!(@one $用意, $属性, 過去の観測は切り替えで引ける);
        全検証!(@one $用意, $属性, 他プロジェクトの機器は出ない);
        全検証!(@one $用意, $属性, 移設された機器も対象に含む);
        全検証!(@one $用意, $属性, 部外者は入れない);
        全検証!(@one $用意, $属性, 未検索と零件を区別する);
        全検証!(@one $用意, $属性, 検索語のワイルドカードを打ち消す);
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

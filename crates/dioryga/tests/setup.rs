//! 初回セットアップの結合テスト（設計書20.8）。

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use dioryga::auth::password::PasswordService;
use dioryga::auth::setup::{self, SetupState};
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, audit_log};
use sea_orm::{DatabaseConnection, EntityTrait};
use std::sync::Arc;
use tower::ServiceExt;

fn 設定() -> Config {
    let mut config = Config::default();
    // テストでは計算量を落とす
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

/// 状態と、生成されたセットアップトークンを返す。
///
/// トークンは `SetupState::initialize` が一度だけ返す。以降 `SetupState` から
/// 読み出す手段は無いため、テストもこの戻り値を使う。
async fn 状態(db: &DatabaseConnection) -> (AppState, Option<String>) {
    let config = 設定();
    let (setup, token) = SetupState::initialize(db).await.unwrap();
    let state = AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
    };
    (state, token)
}

// ---------------------------------------------------------------------------
// テスト本体
// ---------------------------------------------------------------------------

/// 利用者が0件のときだけセットアップが開くこと。
async fn 利用者が居なければセットアップ待ちになる(db: &DatabaseConnection) {
    let (state, token) = 状態(db).await;
    assert!(state.setup.is_pending().await);
    assert!(token.is_some(), "トークンが発行されていません");
}

/// セットアップ待ちの間、他のURLは /setup へ誘導されること。
async fn 保留中は他のurlが誘導される(db: &DatabaseConnection) {
    let (state, _) = 状態(db).await;
    let app = router(state);

    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/setup");
}

/// ヘルスチェックは誘導の対象外であること。
///
/// CIのWindowsジョブが起動直後に叩くため、セットアップ前でも応答する必要がある（2章）。
async fn 保留中でもヘルスチェックは応答する(db: &DatabaseConnection) {
    let (state, _) = 状態(db).await;
    let app = router(state);

    let res = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
}

/// 正しいトークンで最初のSystem Adminを作成できること。
async fn 正しいトークンで管理者を作成できる(db: &DatabaseConnection) {
    let (state, token) = 状態(db).await;
    let token = token.unwrap();

    let user = setup::create_first_admin(
        db,
        &state.setup,
        &state.passwords,
        &token,
        "最初の管理者",
        "first@example.com",
        "correcthorsebatterystaple",
    )
    .await
    .unwrap();

    assert!(user.is_system_admin);
    assert!(
        !state.setup.is_pending().await,
        "トークンが破棄されていません"
    );
}

/// 誤ったトークンでは作成できないこと。
async fn 誤ったトークンでは作成できない(db: &DatabaseConnection) {
    let (state, _token) = 状態(db).await;

    let 結果 = setup::create_first_admin(
        db,
        &state.setup,
        &state.passwords,
        "でたらめなトークン",
        "侵入者",
        "intruder@example.com",
        "correcthorsebatterystaple",
    )
    .await;

    assert!(matches!(結果, Err(setup::SetupError::InvalidToken)));
    assert_eq!(app_user::Entity::find().all(db).await.unwrap().len(), 0);
    assert!(
        state.setup.is_pending().await,
        "失敗でトークンが失効しています"
    );
}

/// ポリシーを満たさないパスワードは拒否されること。
async fn 弱いパスワードでは作成できない(db: &DatabaseConnection) {
    let (state, token) = 状態(db).await;
    let token = token.unwrap();

    let 結果 = setup::create_first_admin(
        db,
        &state.setup,
        &state.passwords,
        &token,
        "管理者",
        "weak@example.com",
        "short",
    )
    .await;

    assert!(matches!(結果, Err(setup::SetupError::Password(_))));
    assert_eq!(app_user::Entity::find().all(db).await.unwrap().len(), 0);
}

/// 作成後は /setup が404を返すこと。
async fn 作成後はセットアップ画面が閉じる(db: &DatabaseConnection) {
    let (state, token) = 状態(db).await;
    let token = token.unwrap();

    setup::create_first_admin(
        db,
        &state.setup,
        &state.passwords,
        &token,
        "管理者",
        "closed@example.com",
        "correcthorsebatterystaple",
    )
    .await
    .unwrap();

    let res = router(state)
        .oneshot(
            Request::builder()
                .uri("/setup")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// 監査ログが「作成された当人」を主体として記録されること（設計書20.8）。
async fn 監査ログの主体は作成された当人になる(db: &DatabaseConnection) {
    let (state, token) = 状態(db).await;
    let token = token.unwrap();

    let user = setup::create_first_admin(
        db,
        &state.setup,
        &state.passwords,
        &token,
        "管理者",
        "audit-setup@example.com",
        "correcthorsebatterystaple",
    )
    .await
    .unwrap();

    let 記録 = audit_log::Entity::find().all(db).await.unwrap();
    assert_eq!(記録.len(), 1);
    assert_eq!(記録[0].user_id, user.id);
    assert_eq!(記録[0].action, "insert");
    // パスワードハッシュは伏せられている
    assert!(!記録[0].after_json.as_ref().unwrap().contains("argon2"));
}

/// 既に利用者が居ればセットアップは開かないこと。
async fn 利用者が居ればセットアップは開かない(db: &DatabaseConnection) {
    let (state, token) = 状態(db).await;
    let token = token.unwrap();
    setup::create_first_admin(
        db,
        &state.setup,
        &state.passwords,
        &token,
        "管理者",
        "existing@example.com",
        "correcthorsebatterystaple",
    )
    .await
    .unwrap();

    // 再起動を想定して状態を作り直す
    let (再起動後, 再発行トークン) = SetupState::initialize(db).await.unwrap();
    assert!(!再起動後.is_pending().await);
    assert!(
        再発行トークン.is_none(),
        "利用者が居るのにトークンが再発行されています"
    );
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        #[tokio::test]
        #[$属性]
        async fn 利用者が居なければセットアップ待ちになる() {
            let db = $用意().await;
            super::利用者が居なければセットアップ待ちになる(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 保留中は他のurlが誘導される() {
            let db = $用意().await;
            super::保留中は他のurlが誘導される(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 保留中でもヘルスチェックは応答する() {
            let db = $用意().await;
            super::保留中でもヘルスチェックは応答する(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 正しいトークンで管理者を作成できる() {
            let db = $用意().await;
            super::正しいトークンで管理者を作成できる(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 誤ったトークンでは作成できない() {
            let db = $用意().await;
            super::誤ったトークンでは作成できない(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 弱いパスワードでは作成できない() {
            let db = $用意().await;
            super::弱いパスワードでは作成できない(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 作成後はセットアップ画面が閉じる() {
            let db = $用意().await;
            super::作成後はセットアップ画面が閉じる(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 監査ログの主体は作成された当人になる() {
            let db = $用意().await;
            super::監査ログの主体は作成された当人になる(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 利用者が居ればセットアップは開かない() {
            let db = $用意().await;
            super::利用者が居ればセットアップは開かない(&db.conn).await;
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

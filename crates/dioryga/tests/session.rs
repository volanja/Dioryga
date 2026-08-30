//! セッションの結合テスト。SQLiteとPostgreSQLの双方で同じ検証を行う。

mod support;

use chrono::{Duration, Utc};
use dioryga::auth::session;
use dioryga::config::SessionConfig;
use entity::app_user;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

fn 設定() -> SessionConfig {
    SessionConfig {
        idle_timeout_secs: 8 * 60 * 60,
        absolute_timeout_secs: 24 * 60 * 60,
        cookie_secure: false,
    }
}

// ---------------------------------------------------------------------------
// テスト本体
// ---------------------------------------------------------------------------

async fn 作成したセッションは検証に成功する(db: &DatabaseConnection) {
    let user = 利用者(db, "session@example.com").await;
    let now = Utc::now();

    let (model, token) = session::create(db, user.id, "127.0.0.1", "test", &設定(), now)
        .await
        .unwrap();

    let 検証結果 = session::validate(db, token.as_str(), &設定(), now)
        .await
        .unwrap();

    assert_eq!(検証結果.map(|s| s.id), Some(model.id));
}

/// 生のトークンはDBに保存されないこと（設計書20.5）。
async fn 生のトークンはdbに残らない(db: &DatabaseConnection) {
    let user = 利用者(db, "raw@example.com").await;
    let now = Utc::now();

    let (model, token) = session::create(db, user.id, "127.0.0.1", "test", &設定(), now)
        .await
        .unwrap();

    assert_ne!(
        model.token_hash,
        token.as_str(),
        "生のトークンがそのまま保存されています"
    );
}

async fn 存在しないトークンは拒否される(db: &DatabaseConnection) {
    let 結果 = session::validate(db, "存在しないトークン", &設定(), Utc::now())
        .await
        .unwrap();
    assert!(結果.is_none());
}

/// 絶対期限を過ぎたセッションは、利用中でも失効すること。
async fn 絶対期限を過ぎると拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "absolute@example.com").await;
    let now = Utc::now();
    let (_, token) = session::create(db, user.id, "127.0.0.1", "test", &設定(), now)
        .await
        .unwrap();

    let 期限後 = now + Duration::seconds(設定().absolute_timeout_secs + 1);
    let 結果 = session::validate(db, token.as_str(), &設定(), 期限後)
        .await
        .unwrap();

    assert!(結果.is_none());
}

/// 最終利用からアイドル期限が過ぎると失効すること。
async fn アイドル期限を過ぎると拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "idle@example.com").await;
    let now = Utc::now();
    let (_, token) = session::create(db, user.id, "127.0.0.1", "test", &設定(), now)
        .await
        .unwrap();

    let 放置後 = now + Duration::seconds(設定().idle_timeout_secs + 1);
    assert!(session::validate(db, token.as_str(), &設定(), 放置後)
        .await
        .unwrap()
        .is_none());
}

/// 利用し続けている限りアイドル期限では切れないこと。
async fn 利用を続ければアイドル期限では切れない(db: &DatabaseConnection) {
    let user = 利用者(db, "touch@example.com").await;
    let now = Utc::now();
    let (model, token) = session::create(db, user.id, "127.0.0.1", "test", &設定(), now)
        .await
        .unwrap();

    // アイドル期限の直前に利用する
    let 直前 = now + Duration::seconds(設定().idle_timeout_secs - 1);
    session::touch(db, model, 直前).await.unwrap();

    // 起点が更新されているので、さらに時間が経っても有効
    let さらに後 = 直前 + Duration::seconds(設定().idle_timeout_secs - 1);
    assert!(session::validate(db, token.as_str(), &設定(), さらに後)
        .await
        .unwrap()
        .is_some());
}

async fn 失効させたセッションは拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "revoke@example.com").await;
    let now = Utc::now();
    let (model, token) = session::create(db, user.id, "127.0.0.1", "test", &設定(), now)
        .await
        .unwrap();

    session::revoke(db, model, now).await.unwrap();

    assert!(session::validate(db, token.as_str(), &設定(), now)
        .await
        .unwrap()
        .is_none());
}

/// パスワード変更時、使用中のセッションだけを残して他を切ること（設計書20.7）。
async fn 使用中以外のセッションを一括で失効できる(db: &DatabaseConnection) {
    let user = 利用者(db, "bulk@example.com").await;
    let now = Utc::now();

    let (使用中, 使用中トークン) =
        session::create(db, user.id, "127.0.0.1", "現在の端末", &設定(), now)
            .await
            .unwrap();
    let (_, 別端末トークン) = session::create(db, user.id, "10.0.0.1", "別の端末", &設定(), now)
        .await
        .unwrap();
    let (_, さらに別) = session::create(db, user.id, "10.0.0.2", "3台目", &設定(), now)
        .await
        .unwrap();

    let 失効数 = session::revoke_all_except(db, user.id, Some(使用中.id), now)
        .await
        .unwrap();
    assert_eq!(失効数, 2);

    // 変更した本人はログインしたまま
    assert!(session::validate(db, 使用中トークン.as_str(), &設定(), now)
        .await
        .unwrap()
        .is_some());
    // 他の端末は切れる
    assert!(session::validate(db, 別端末トークン.as_str(), &設定(), now)
        .await
        .unwrap()
        .is_none());
    assert!(session::validate(db, さらに別.as_str(), &設定(), now)
        .await
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証用".to_owned()),
        email: Set(email.to_owned()),
        password_hash: Set("dummy".to_owned()),
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
        #[tokio::test]
        #[$属性]
        async fn 作成したセッションは検証に成功する() {
            let db = $用意().await;
            super::作成したセッションは検証に成功する(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 生のトークンはdbに残らない() {
            let db = $用意().await;
            super::生のトークンはdbに残らない(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 存在しないトークンは拒否される() {
            let db = $用意().await;
            super::存在しないトークンは拒否される(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 絶対期限を過ぎると拒否される() {
            let db = $用意().await;
            super::絶対期限を過ぎると拒否される(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn アイドル期限を過ぎると拒否される() {
            let db = $用意().await;
            super::アイドル期限を過ぎると拒否される(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 利用を続ければアイドル期限では切れない() {
            let db = $用意().await;
            super::利用を続ければアイドル期限では切れない(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 失効させたセッションは拒否される() {
            let db = $用意().await;
            super::失効させたセッションは拒否される(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 使用中以外のセッションを一括で失効できる() {
            let db = $用意().await;
            super::使用中以外のセッションを一括で失効できる(&db.conn).await;
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

// ---------------------------------------------------------------------------
// レート制限（設計書20.6）
// ---------------------------------------------------------------------------

mod rate_limit_tests {
    use super::*;
    use dioryga::auth::rate_limit::{self, Decision, MAX_FAILURES};

    async fn 失敗を重ねると一時停止する(db: &DatabaseConnection) {
        let now = Utc::now();
        let email = "throttle@example.com";

        assert_eq!(
            rate_limit::check(db, email, "192.0.2.1", now)
                .await
                .unwrap(),
            Decision::Allow
        );

        for _ in 0..MAX_FAILURES {
            rate_limit::record(db, email, "192.0.2.1", false, now)
                .await
                .unwrap();
        }

        assert_eq!(
            rate_limit::check(db, email, "192.0.2.1", now)
                .await
                .unwrap(),
            Decision::Throttled
        );
    }

    /// 停止時間が過ぎれば再び受け付けること。**恒久ロックにしない**
    /// （任意のメールで他人をロックアウトできるDoSを避けるため。設計書20.6）。
    async fn 時間が経てば再び受け付ける(db: &DatabaseConnection) {
        let now = Utc::now();
        let email = "recover@example.com";

        for _ in 0..MAX_FAILURES {
            rate_limit::record(db, email, "192.0.2.2", false, now)
                .await
                .unwrap();
        }
        assert_eq!(
            rate_limit::check(db, email, "192.0.2.2", now)
                .await
                .unwrap(),
            Decision::Throttled
        );

        let 停止後 = now + Duration::seconds(rate_limit::LOCKOUT_SECS + 1);
        assert_eq!(
            rate_limit::check(db, email, "192.0.2.2", 停止後)
                .await
                .unwrap(),
            Decision::Allow
        );
    }

    /// IPアドレス単位でも数えること。IPを変えずに別アカウントを試す攻撃に対応する。
    async fn ipアドレス単位でも停止する(db: &DatabaseConnection) {
        let now = Utc::now();

        for i in 0..MAX_FAILURES {
            rate_limit::record(
                db,
                &format!("victim{i}@example.com"),
                "192.0.2.3",
                false,
                now,
            )
            .await
            .unwrap();
        }

        // メールアドレスは初めてでも、IPが同じなら停止する
        assert_eq!(
            rate_limit::check(db, "another@example.com", "192.0.2.3", now)
                .await
                .unwrap(),
            Decision::Throttled
        );
    }

    /// 成功した試行は失敗回数に数えないこと。
    async fn 成功は回数に数えない(db: &DatabaseConnection) {
        let now = Utc::now();
        let email = "success@example.com";

        for _ in 0..(MAX_FAILURES + 3) {
            rate_limit::record(db, email, "192.0.2.4", true, now)
                .await
                .unwrap();
        }

        assert_eq!(
            rate_limit::check(db, email, "192.0.2.4", now)
                .await
                .unwrap(),
            Decision::Allow
        );
    }

    macro_rules! 全検証 {
        ($用意:path, $属性:meta) => {
            #[tokio::test]
            #[$属性]
            async fn 失敗を重ねると一時停止する() {
                let db = $用意().await;
                super::失敗を重ねると一時停止する(&db.conn).await;
            }

            #[tokio::test]
            #[$属性]
            async fn 時間が経てば再び受け付ける() {
                let db = $用意().await;
                super::時間が経てば再び受け付ける(&db.conn).await;
            }

            #[tokio::test]
            #[$属性]
            async fn ipアドレス単位でも停止する() {
                let db = $用意().await;
                super::ipアドレス単位でも停止する(&db.conn).await;
            }

            #[tokio::test]
            #[$属性]
            async fn 成功は回数に数えない() {
                let db = $用意().await;
                super::成功は回数に数えない(&db.conn).await;
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
}

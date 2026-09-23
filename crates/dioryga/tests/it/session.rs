//! セッションの結合テスト。SQLiteとPostgreSQLの双方で同じ検証を行う。

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
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
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
    use dioryga::auth::rate_limit::{
        self, Decision, FREE_FAILURES, IP_MAX_FAILURES, IP_WINDOW_SECS,
    };

    /// 時刻を固定して、失敗を1秒ずつずらして積む。
    async fn 失敗を積む(
        db: &DatabaseConnection,
        username: &str,
        ip: &str,
        回数: u64,
        起点: chrono::DateTime<Utc>,
    ) {
        for i in 0..回数 {
            rate_limit::record(db, username, ip, false, 起点 + Duration::seconds(i as i64))
                .await
                .unwrap();
        }
    }

    async fn 判定(
        db: &DatabaseConnection,
        username: &str,
        ip: &str,
        now: chrono::DateTime<Utc>,
    ) -> Decision {
        rate_limit::check(db, username, ip, now).await.unwrap()
    }

    /// **3回までは待ちなし**（打ち間違いで待たされない）。
    async fn 三回までは待たせない(db: &DatabaseConnection) {
        let t0 = Utc::now();
        失敗を積む(db, "yarigatake", "192.0.2.1", FREE_FAILURES, t0).await;
        let 直後 = t0 + Duration::seconds(FREE_FAILURES as i64 - 1);
        assert_eq!(
            判定(db, "yarigatake", "192.0.2.1", 直後).await,
            Decision::Allow
        );
    }

    /// **4回目の失敗から待ち時間が掛かり、倍になり、60秒で頭打ちになる**（設計書20.6）。
    ///
    /// 待ち時間は最後の失敗から数える。受付を止めるのではなく、
    /// 「次に受け付ける時刻」までを断る。
    async fn 待ち時間は倍になり六十秒で頭打ち(db: &DatabaseConnection) {
        let t0 = Utc::now();
        let mut 最後 = t0;
        for (連続, 待ち) in [
            (4u64, 1i64),
            (5, 2),
            (6, 4),
            (7, 8),
            (8, 16),
            (9, 32),
            (10, 60),
            (11, 60),
        ] {
            最後 = t0 + Duration::seconds(連続 as i64 * 100);
            rate_limit::record(db, "hotaka", "192.0.2.2", false, 最後)
                .await
                .unwrap();
            // 最初の3回は、この前にまとめて積んでおく
            if 連続 == 4 {
                失敗を積む(db, "hotaka", "192.0.2.2", 3, t0).await;
            }

            // 直後は断り、残り秒数を返す
            assert_eq!(
                判定(db, "hotaka", "192.0.2.2", 最後).await,
                Decision::Throttled {
                    retry_after_secs: 待ち
                },
                "{連続}回目の失敗の直後"
            );
            // 待ち時間が過ぎれば受け付ける
            assert_eq!(
                判定(db, "hotaka", "192.0.2.2", 最後 + Duration::seconds(待ち)).await,
                Decision::Allow,
                "{連続}回目の失敗から{待ち}秒後"
            );
        }
        let _ = 最後;
    }

    /// 残り秒数は時間とともに減ること。
    async fn 残り秒数は減っていく(db: &DatabaseConnection) {
        let t0 = Utc::now();
        失敗を積む(db, "hakuba", "192.0.2.3", 10, t0).await;
        let 最後 = t0 + Duration::seconds(9);

        assert_eq!(
            判定(db, "hakuba", "192.0.2.3", 最後 + Duration::seconds(45)).await,
            Decision::Throttled {
                retry_after_secs: 15
            }
        );
    }

    /// **ログインに成功したら数え直すこと。**
    async fn 成功すると数え直す(db: &DatabaseConnection) {
        let t0 = Utc::now();
        失敗を積む(db, "tateyama", "192.0.2.4", 8, t0).await;
        rate_limit::record(
            db,
            "tateyama",
            "192.0.2.4",
            true,
            t0 + Duration::seconds(100),
        )
        .await
        .unwrap();
        // 成功の後の失敗は1回目から数える
        rate_limit::record(
            db,
            "tateyama",
            "192.0.2.4",
            false,
            t0 + Duration::seconds(101),
        )
        .await
        .unwrap();

        assert_eq!(
            判定(db, "tateyama", "192.0.2.4", t0 + Duration::seconds(101)).await,
            Decision::Allow
        );
    }

    /// **IPを変えても、アカウント単位で数えること**（設計書20.6、OWASP）。
    ///
    /// 「ユーザー名＋IP」の組で数えると、IPを変えながら1つのアカウントを
    /// 狙う攻撃を止められない。
    async fn ipを変えてもアカウント単位で数える(db: &DatabaseConnection) {
        let t0 = Utc::now();
        for i in 0..5 {
            rate_limit::record(
                db,
                "kita",
                &format!("198.51.100.{i}"),
                false,
                t0 + Duration::seconds(i),
            )
            .await
            .unwrap();
        }
        assert!(matches!(
            判定(db, "kita", "203.0.113.99", t0 + Duration::seconds(4)).await,
            Decision::Throttled { .. }
        ));
    }

    /// **IP単位は15分に50回で止め、49回では止めないこと**（設計書20.6）。
    ///
    /// 社内では多数の利用者が同じ出口IPを共有する。アカウント単位と同じ
    /// 小さな上限だと、1人の打ち間違いで事務所全体が止まる。
    async fn ip単位は五十回で止める(db: &DatabaseConnection) {
        let t0 = Utc::now();
        // 別々のアカウントで失敗させ、アカウント単位の待ちには掛からないようにする
        for i in 0..(IP_MAX_FAILURES - 1) {
            rate_limit::record(db, &format!("victim{i}"), "192.0.2.5", false, t0)
                .await
                .unwrap();
        }
        assert_eq!(
            判定(db, "newcomer", "192.0.2.5", t0).await,
            Decision::Allow,
            "49回で止まっています"
        );

        rate_limit::record(db, "victim49", "192.0.2.5", false, t0)
            .await
            .unwrap();
        assert!(matches!(
            判定(db, "newcomer", "192.0.2.5", t0).await,
            Decision::Throttled { .. }
        ));

        // 期間を過ぎれば再び受け付ける
        assert_eq!(
            判定(
                db,
                "newcomer",
                "192.0.2.5",
                t0 + Duration::seconds(IP_WINDOW_SECS + 1)
            )
            .await,
            Decision::Allow
        );
    }

    /// 成功した試行は失敗回数に数えないこと。
    async fn 成功は回数に数えない(db: &DatabaseConnection) {
        let now = Utc::now();
        for _ in 0..(FREE_FAILURES + 5) {
            rate_limit::record(db, "senjo", "192.0.2.6", true, now)
                .await
                .unwrap();
        }
        assert_eq!(判定(db, "senjo", "192.0.2.6", now).await, Decision::Allow);
    }

    macro_rules! 全検証 {
        ($用意:path, $属性:meta) => {
            全検証!(@one $用意, $属性, 三回までは待たせない);
            全検証!(@one $用意, $属性, 待ち時間は倍になり六十秒で頭打ち);
            全検証!(@one $用意, $属性, 残り秒数は減っていく);
            全検証!(@one $用意, $属性, 成功すると数え直す);
            全検証!(@one $用意, $属性, ipを変えてもアカウント単位で数える);
            全検証!(@one $用意, $属性, ip単位は五十回で止める);
            全検証!(@one $用意, $属性, 成功は回数に数えない);
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
}

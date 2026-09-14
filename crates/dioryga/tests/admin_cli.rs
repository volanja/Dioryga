//! `dioryga admin create` の結合テスト（設計書20.1、20.8、#129、#136）。
//!
//! 対話でも標準入力でも同じ本体（`admin::作成する`）を通る。ここではその本体を
//! 直接呼び、**入力の受け取り方によらず検査が同じであること**を確かめる。
//! 標準入力の読み方そのものは `admin.rs` の単体テストが見る。

mod support;

use dioryga::admin;
use dioryga::auth::password::PasswordService;
use dioryga::config::Config;
use entity::app_user;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};

const パスワード: &str = "Tanigawa-Bridge-7391";

/// 作成した利用者が System Admin で、**そのパスワードでログインできること。**
///
/// 標準入力から渡したパスワードの改行が残っていたり、別の値でハッシュ化されて
/// いたりすると、作成には成功してもログインできない。
async fn system_adminを作成できる(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    let user = admin::作成する(
        db,
        &passwords,
        "yarigatake",
        Some("yarigatake@example.invalid"),
        "槍ヶ岳 大川",
        パスワード,
    )
    .await
    .unwrap();

    assert!(user.is_system_admin, "System Adminになっていません");
    assert!(!user.must_change_password);
    assert_eq!(user.email.as_deref(), Some("yarigatake@example.invalid"));
    assert!(
        passwords
            .verify(パスワード, &user.password_hash)
            .await
            .unwrap()
            .is_some(),
        "渡したパスワードでログインできません"
    );
}

/// **メールアドレス無しで作成でき、ユーザー名は小文字で保存されること**（設計書20.1）。
async fn メールアドレス無しでも作成でき小文字で保存する(
    db: &DatabaseConnection,
) {
    let passwords = パスワード処理();
    let user = admin::作成する(db, &passwords, "Hotaka.Kojo", None, "穂高 古城", パスワード)
        .await
        .unwrap();

    assert_eq!(user.username, "hotaka.kojo");
    assert_eq!(user.email, None);
}

/// **パスワードポリシーに反すれば作らないこと。**
///
/// 対話と同じ検査を通る。スクリプトからなら弱いパスワードで作れる、という
/// 抜け道にしない。
async fn ポリシーに反するパスワードは拒否する(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    let 結果 = admin::作成する(db, &passwords, "hakuba", None, "白馬 石垣", "short").await;

    assert!(結果.is_err(), "短いパスワードで作成できてしまいます");
    assert_eq!(人数(db, "hakuba").await, 0);
}

/// **規則に合わないユーザー名では作らないこと**（設計書20.1）。
async fn 規則外のユーザー名は拒否する(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    for 誤り in ["t", "槍ヶ岳", "_tateyama", "tate yama"] {
        let 結果 = admin::作成する(db, &passwords, 誤り, None, "立山 滝見", パスワード).await;
        assert!(結果.is_err(), "「{誤り}」で作成できてしまいます");
    }
    assert_eq!(app_user::Entity::find().count(db).await.unwrap(), 0);
}

/// **同じユーザー名は、大文字小文字が違っても作らないこと**（設計書20.1）。
async fn 既存のユーザー名は拒否する(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    admin::作成する(
        db,
        &passwords,
        "yatsugatake",
        None,
        "八ヶ岳 天守",
        パスワード,
    )
    .await
    .unwrap();

    let 二度目 = admin::作成する(
        db,
        &passwords,
        "YatsuGatake",
        None,
        "八ヶ岳 大川",
        パスワード,
    )
    .await;

    assert!(二度目.is_err());
    assert_eq!(人数(db, "yatsugatake").await, 1);
}

/// **メールアドレスは任意だが、値がある場合は他の利用者と重複させないこと**（設計書20.1）。
async fn 既存のメールアドレスは拒否する(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    admin::作成する(
        db,
        &passwords,
        "kita",
        Some("shared@example.invalid"),
        "北岳 大川",
        パスワード,
    )
    .await
    .unwrap();

    let 二人目 = admin::作成する(
        db,
        &passwords,
        "kaikoma",
        Some("SHARED@example.invalid"),
        "甲斐駒 古城",
        パスワード,
    )
    .await;

    assert!(
        二人目.is_err(),
        "同じメールアドレスで2人目を作成できてしまいます"
    );
    assert_eq!(人数(db, "kaikoma").await, 0);
}

/// 表示名が空なら作らないこと。
async fn 表示名が空なら拒否する(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    let 結果 = admin::作成する(db, &passwords, "senjo", None, "  ", パスワード).await;

    assert!(結果.is_err());
    assert_eq!(人数(db, "senjo").await, 0);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn パスワード処理() -> PasswordService {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    PasswordService::new(config.password).unwrap()
}

async fn 人数(db: &DatabaseConnection, username: &str) -> u64 {
    app_user::Entity::find()
        .filter(app_user::Column::Username.eq(username))
        .count(db)
        .await
        .unwrap()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, system_adminを作成できる);
        全検証!(@one $用意, $属性, メールアドレス無しでも作成でき小文字で保存する);
        全検証!(@one $用意, $属性, ポリシーに反するパスワードは拒否する);
        全検証!(@one $用意, $属性, 規則外のユーザー名は拒否する);
        全検証!(@one $用意, $属性, 既存のユーザー名は拒否する);
        全検証!(@one $用意, $属性, 既存のメールアドレスは拒否する);
        全検証!(@one $用意, $属性, 表示名が空なら拒否する);
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

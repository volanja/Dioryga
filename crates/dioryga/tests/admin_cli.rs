//! `dioryga admin create` の結合テスト（設計書20.8、#129）。
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
        "yarigatake@example.invalid",
        "槍ヶ岳 大川",
        パスワード,
    )
    .await
    .unwrap();

    assert!(user.is_system_admin, "System Adminになっていません");
    assert!(!user.must_change_password);
    assert!(
        passwords
            .verify(パスワード, &user.password_hash)
            .await
            .unwrap()
            .is_some(),
        "渡したパスワードでログインできません"
    );
}

/// **パスワードポリシーに反すれば作らないこと。**
///
/// 対話と同じ検査を通る。スクリプトからなら弱いパスワードで作れる、という
/// 抜け道にしない。
async fn ポリシーに反するパスワードは拒否する(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    let 結果 = admin::作成する(
        db,
        &passwords,
        "hotaka@example.invalid",
        "穂高 古城",
        "short",
    )
    .await;

    assert!(結果.is_err(), "短いパスワードで作成できてしまいます");
    assert_eq!(人数(db, "hotaka@example.invalid").await, 0);
}

/// 同じメールアドレスの利用者は作らないこと。
async fn 既存のメールアドレスは拒否する(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    admin::作成する(
        db,
        &passwords,
        "hakuba@example.invalid",
        "白馬 石垣",
        パスワード,
    )
    .await
    .unwrap();

    let 二度目 = admin::作成する(
        db,
        &passwords,
        "hakuba@example.invalid",
        "白馬 滝見",
        パスワード,
    )
    .await;

    assert!(二度目.is_err());
    assert_eq!(人数(db, "hakuba@example.invalid").await, 1);
}

/// 表示名が空なら作らないこと。
async fn 表示名が空なら拒否する(db: &DatabaseConnection) {
    let passwords = パスワード処理();
    let 結果 = admin::作成する(db, &passwords, "tateyama@example.invalid", "  ", パスワード).await;

    assert!(結果.is_err());
    assert_eq!(人数(db, "tateyama@example.invalid").await, 0);
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

async fn 人数(db: &DatabaseConnection, email: &str) -> u64 {
    app_user::Entity::find()
        .filter(app_user::Column::Email.eq(email))
        .count(db)
        .await
        .unwrap()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, system_adminを作成できる);
        全検証!(@one $用意, $属性, ポリシーに反するパスワードは拒否する);
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

//! `docs/examples/` の取込例の結合テスト（#194）。
//!
//! **例は書式の説明であり、そのまま流せなければ意味が無い。**READMEから辿られる
//! 場所にあるのに、壊れていても誰も気付けない。組織データ → カタログ →
//! プロジェクトのデータの順に、例のコメントに書いた手順どおり流す。

use std::path::{Path, PathBuf};

use chrono::Utc;
use dioryga::import::run;
use dioryga::import::{Outcome, Report};
use entity::{
    app_user, device, device_mount, ip_address, maintenance_contract_item, milestone,
    mount_container, part_instance, project, work_order, work_order_approval,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};

/// **例をすべて流すと、誤りも警告も出ないこと。**
///
/// 警告も許さない。例に警告が出ると、読んだ人は警告の出る書き方を手本にする。
async fn 例をすべて取り込める(db: &DatabaseConnection) {
    let 取込者 = 例を流す(db).await;

    // 誤りが出ないだけでなく、書いた行が入っていること
    assert_eq!(device::Entity::find().count(db).await.unwrap(), 5);
    assert_eq!(device_mount::Entity::find().count(db).await.unwrap(), 5);
    assert_eq!(part_instance::Entity::find().count(db).await.unwrap(), 4);
    assert_eq!(ip_address::Entity::find().count(db).await.unwrap(), 4);
    assert_eq!(
        maintenance_contract_item::Entity::find()
            .count(db)
            .await
            .unwrap(),
        3
    );
    assert_eq!(milestone::Entity::find().count(db).await.unwrap(), 3);
    assert_eq!(work_order::Entity::find().count(db).await.unwrap(), 4);
    let 承認済み = work_order_approval::Entity::find()
        .filter(work_order_approval::Column::Status.eq("approved"))
        .count(db)
        .await
        .unwrap();
    assert_eq!(承認済み, 2);

    // **宣言的な取込（23.1）なので、二度流しても何も変わらない**
    for path in ["catalog.yaml", "instances/manifest.yaml"] {
        let 実行 = run::run(db, &例(path), &取込者, true).await.unwrap();
        for outcome in [Outcome::Created, Outcome::Updated] {
            assert_eq!(
                実行.report.count(outcome),
                0,
                "{path} を二度流すと {outcome:?} が出ます\n{}",
                詳細(&実行.report)
            );
        }
    }
}

/// 例のコメントに書いた手順どおりに流し、プロジェクトの取込者を返す。
async fn 例を流す(db: &DatabaseConnection) -> String {
    利用者(db, "admin", true).await;
    流す(db, "organization/manifest.yaml", "admin").await;

    // `members.csv` で MIZUBASHO-01 の正管理者になっている
    let 取込者 = "hotaka";
    流す(db, "catalog.yaml", 取込者).await;

    // **什器は取込では作れない。**例のコメントどおり、画面で作った状態にする
    什器(db, "MIZUBASHO-01", "A01", 取込者).await;
    流す(db, "instances/manifest.yaml", 取込者).await;

    取込者.to_owned()
}

async fn 流す(db: &DatabaseConnection, path: &str, as_user: &str) {
    let 実行 = run::run(db, &例(path), as_user, true)
        .await
        .unwrap_or_else(|e| panic!("{path} を流せません: {e}"));
    let report = &実行.report;
    assert!(
        report.errors().next().is_none() && report.warnings().next().is_none(),
        "{path} で誤りか警告が出ます\n{}",
        詳細(report)
    );
}

fn 例(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/examples")
        .join(path)
}

fn 詳細(report: &Report) -> String {
    report
        .entries
        .iter()
        .filter(|e| e.outcome != Outcome::Unchanged)
        .map(|e| format!("{:?} {} {}", e.outcome, e.target, e.detail))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn 利用者(db: &DatabaseConnection, username: &str, system_admin: bool) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(format!("{username} の表示名")),
        username: Set(username.to_owned()),
        email: Set(None),
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

async fn 什器(db: &DatabaseConnection, code: &str, name: &str, created_by: &str) {
    let p = project::Entity::find()
        .filter(project::Column::Code.eq(code))
        .one(db)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("プロジェクト {code} がありません"));
    let u = app_user::Entity::find()
        .filter(app_user::Column::Username.eq(created_by))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    mount_container::ActiveModel {
        name: Set(name.to_owned()),
        container_type: Set("Rack".to_owned()),
        location_type: Set("Project".to_owned()),
        location_id: Set(p.id),
        capacity: Set(Some(42)),
        created_by: Set(u.id),
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
        全検証!(@one $用意, $属性, 例をすべて取り込める);
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

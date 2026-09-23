//! マイルストーンと変更管理チケットの取込の結合テスト（設計書23.5、10.4、11章）。
//!
//! # 何を確かめているか
//!
//! **二度流しても増えないこと**（23.1）。この2つの表は `uid` も `external_id` も
//! 持たないまま設計しており、#140・#141 で足した。**突合できなければ毎回新規に
//! なる**ため、宣言的な取込が成り立たない。ここが崩れたら取込の前提が崩れる。
//!
//! **状態を書かれたまま入れること。**11.2の遷移を取込で再生しない。移行で
//! 持ち込みたいのは「いまどの状態か」である。
//!
//! **チケットを作ると承認行が起きること**（11.4-7）。画面から起票したものと
//! 形が違うチケットが取込からだけ生まれないようにする。
//!
//! **承認が揃わないまま進んでいるチケットを警告すること。**止めはしない
//! （不変条件6）——移行元に承認の記録が無いことは実際にある。

mod support;

use chrono::{Duration, NaiveDate, Utc};
use dioryga::import::{workflow, Outcome};
use dioryga::repository::{Actor, AuditedTx};
use entity::{
    app_user, device, device_assignment, milestone, project, project_member, work_order,
    work_order_approval,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, PaginatorTrait, Set};

const マイルストーン見出し: &str =
    "uid,external_id,milestone_type,planned_date,actual_date,status,description\n";
const チケット見出し: &str = "uid,external_id,work_type,title,description,device_uid,device_external_id,device_hostname,device_serial_number,target_project,primary_assignee,secondary_assignee,due_date,status,cancelled_reason\n";
const 承認見出し: &str = "work_order,required_project,approver,status\n";

fn マイルストーン(rows: &[&str]) -> Vec<workflow::MilestoneRow> {
    workflow::parse_milestones(&format!("{マイルストーン見出し}{}", rows.join("\n"))).unwrap()
}
fn チケット(rows: &[&str]) -> Vec<workflow::WorkOrderRow> {
    workflow::parse_work_orders(&format!("{チケット見出し}{}", rows.join("\n"))).unwrap()
}
fn 承認(rows: &[&str]) -> Vec<workflow::ApprovalRow> {
    workflow::parse_approvals(&format!("{承認見出し}{}", rows.join("\n"))).unwrap()
}

// ---------------------------------------------------------------------------
// マイルストーン（設計書10.4）
// ---------------------------------------------------------------------------

/// **登録でき、二度流しても増えないこと**（23.1）。
async fn マイルストーンを登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "マイルストーン").await;
    let rows = マイルストーン(&[",MS-1,ServiceStart,2026-10-01,,planned,本番開始"]);

    let tx = 取込(db).await;
    let r = workflow::マイルストーンを取り込む(&tx, 場.project.id, &rows)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Created), 1, "{r}");

    let m = milestone::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(m.milestone_type, "ServiceStart");
    assert_eq!(
        m.planned_date,
        NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
    );
    assert_eq!(m.status, "planned");
    assert_eq!(m.description, "本番開始");
    assert_eq!(m.external_id.as_deref(), Some("MS-1"));
    assert!(!m.uid.is_empty(), "uid を採番していません");

    let tx = 取込(db).await;
    let r = workflow::マイルストーンを取り込む(&tx, 場.project.id, &rows)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Unchanged), 1, "{r}");
    assert_eq!(milestone::Entity::find().count(db).await.unwrap(), 1);
}

/// **予定日を動かしても同じ行を更新すること。**日付をキーに含めていたら別物になる。
async fn 予定日を動かしても同じ行(db: &DatabaseConnection) {
    let 場 = 舞台(db, "予定変更").await;
    let tx = 取込(db).await;
    workflow::マイルストーンを取り込む(
        &tx,
        場.project.id,
        &マイルストーン(&[",MS-1,ServiceStart,2026-10-01,,planned,本番開始"]),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let tx = 取込(db).await;
    let r = workflow::マイルストーンを取り込む(
        &tx,
        場.project.id,
        &マイルストーン(&[",MS-1,ServiceStart,2026-11-15,,planned,本番開始"]),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Updated), 1, "{r}");
    assert_eq!(milestone::Entity::find().count(db).await.unwrap(), 1);
    let m = milestone::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(
        m.planned_date,
        NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()
    );
}

/// **`uid` も `external_id` も無い行はエラー**（23.5）。毎回新規になる。
async fn 識別子の無い行はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "識別子なし").await;
    let tx = 取込(db).await;
    let r = workflow::マイルストーンを取り込む(
        &tx,
        場.project.id,
        &マイルストーン(
            &",,ServiceStart,2026-10-01,,planned,"
                .split('\n')
                .collect::<Vec<_>>(),
        ),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(r.count(Outcome::Error), 1, "{r}");
    assert!(
        r.errors()
            .any(|e| e.detail.contains("突き合わせられません")),
        "{r}"
    );
}

/// **語彙外の種別は拒否する**（10.4）。既定へ寄せない。
async fn 語彙外の種別はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "語彙外").await;
    let tx = 取込(db).await;
    let r = workflow::マイルストーンを取り込む(
        &tx,
        場.project.id,
        &マイルストーン(&[",MS-1,ServiceOpen,2026-10-01,,planned,"]),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// 変更管理チケット（設計書11章）
// ---------------------------------------------------------------------------

/// **状態を書かれたまま入れ、二度流しても増えないこと。**
async fn チケットを登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "チケット").await;
    let rows =
        チケット(
            &[",WO-1,Repair,電源交換,PSUが落ちる,,,web01,,,担当,,2026-10-01,in_progress,"],
        );

    let tx = 取込(db).await;
    let r = workflow::チケットを取り込む(&tx, 場.project.id, &rows, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Created), 1, "{r}");

    let w = work_order::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(w.work_type, "Repair");
    assert_eq!(w.title, "電源交換");
    assert_eq!(w.status, "in_progress", "状態を書き換えています");
    assert_eq!(w.device_id, Some(場.device.id));
    assert_eq!(w.primary_assignee_id, Some(場.assignee));
    assert_eq!(w.due_date, NaiveDate::from_ymd_opt(2026, 10, 1));
    // **実行済みの時刻が入る**（11.2）
    assert!(w.executed_at.is_some());
    assert!(w.completed_at.is_none());

    let tx = 取込(db).await;
    let r = workflow::チケットを取り込む(&tx, 場.project.id, &rows, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Unchanged), 1, "{r}");
    assert_eq!(work_order::Entity::find().count(db).await.unwrap(), 1);
}

/// **作ると承認行が起きること**（11.4-7）。画面から起票したものと同じ形にする。
async fn チケットを作ると承認行が起きる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "承認行").await;
    let tx = 取込(db).await;
    workflow::チケットを取り込む(
        &tx,
        場.project.id,
        &チケット(&[",WO-1,Repair,電源交換,,,,web01,,,担当,,,planned,"]),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let a = work_order_approval::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.required_project_id, 場.project.id);
    assert_eq!(a.status, "pending");
    assert!(a.approver_id.is_none());
    // **取込では立てない**（11.4-9）。判定に要る事実がファイルに無い
    assert!(!a.self_approved);
}

/// **承認行を取り込み、チケットの承認として読めること。**
async fn 承認を取り込める(db: &DatabaseConnection) {
    let 場 = 舞台(db, "承認").await;
    let tx = 取込(db).await;
    workflow::チケットを取り込む(
        &tx,
        場.project.id,
        &チケット(&[",WO-1,Repair,電源交換,,,,web01,,,担当,,,approved,"]),
        Utc::now(),
    )
    .await
    .unwrap();
    let r = workflow::承認を取り込む(
        &tx,
        場.project.id,
        &承認(&[&format!("WO-1,{},承認,approved", 場.project.name)]),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // 起票時に起きた pending の行を更新する
    assert_eq!(r.count(Outcome::Updated), 1, "{r}");
    assert_eq!(
        work_order_approval::Entity::find().count(db).await.unwrap(),
        1,
        "承認行が増えています"
    );
    let a = work_order_approval::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.status, "approved");
    assert_eq!(a.approver_id, Some(場.approver));
    assert!(a.approved_at.is_some());
}

/// **承認が揃わないまま進んでいたら警告する**（11.4-7、不変条件6）。止めない。
async fn 承認が揃わない完了は警告(db: &DatabaseConnection) {
    let 場 = 舞台(db, "不揃い").await;
    let tx = 取込(db).await;
    workflow::チケットを取り込む(
        &tx,
        場.project.id,
        &チケット(&[",WO-1,Repair,電源交換,,,,web01,,,担当,,,completed,"]),
        Utc::now(),
    )
    .await
    .unwrap();
    // approvals.csv を渡さない＝起票時の pending が残ったまま
    let r = workflow::承認を取り込む(&tx, 場.project.id, &[], Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Warning), 1, "{r}");
    assert!(
        r.warnings().any(|e| e.detail.contains("揃っていない承認")),
        "{r}"
    );
    // **止めない。**チケットは入っている
    assert_eq!(work_order::Entity::find().count(db).await.unwrap(), 1);
}

/// **Transfer には移譲先が要り、それ以外では書けない**（11.5）。
async fn 移譲先の規則(db: &DatabaseConnection) {
    let 場 = 舞台(db, "移譲").await;

    let tx = 取込(db).await;
    let r = workflow::チケットを取り込む(
        &tx,
        場.project.id,
        &チケット(&[",WO-1,Transfer,移譲,,,,web01,,,担当,,,planned,"]),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");

    let tx = 取込(db).await;
    let r = workflow::チケットを取り込む(
        &tx,
        場.project.id,
        &チケット(&[&format!(
            ",WO-2,Repair,修理,,,,web01,,{},担当,,,planned,",
            場.project.name
        )]),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **中止の理由を書けるのは中止のときだけ**（11.4-8）。
async fn 中止理由は中止のときだけ(db: &DatabaseConnection) {
    let 場 = 舞台(db, "中止理由").await;
    let tx = 取込(db).await;
    let r = workflow::チケットを取り込む(
        &tx,
        場.project.id,
        &チケット(&[",WO-1,Repair,修理,,,,web01,,,担当,,,planned,やめた"]),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **メンバーでない担当者は警告して取り込む**（不変条件6）。
///
/// 移行元の担当者が今もメンバーとは限らない。拒否すると事実を記録できなくなる。
async fn メンバー外の担当者は警告(db: &DatabaseConnection) {
    let 場 = 舞台(db, "メンバー外").await;
    let tx = 取込(db).await;
    let r = workflow::チケットを取り込む(
        &tx,
        場.project.id,
        &チケット(&[",WO-1,Repair,修理,,,,web01,,,部外,,,planned,"]),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Warning), 1, "{r}");
    assert_eq!(work_order::Entity::find().count(db).await.unwrap(), 1);
    let w = work_order::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(w.primary_assignee_id, Some(場.外部));
}

/// **知らない機器はエラー**（23.2）。黙って機器なしのチケットにしない。
async fn 知らない機器はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "知らない機器").await;
    let tx = 取込(db).await;
    let r = workflow::チケットを取り込む(
        &tx,
        場.project.id,
        &チケット(&[",WO-1,Repair,修理,,,,db99,,,担当,,,planned,"]),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// 舞台
// ---------------------------------------------------------------------------

struct 舞台情報 {
    project: project::Model,
    device: device::Model,
    /// このプロジェクトのメンバー。
    assignee: i32,
    approver: i32,
    /// メンバーではない利用者。
    外部: i32,
}

async fn 取込(db: &DatabaseConnection) -> AuditedTx {
    AuditedTx::begin(db, Actor::Import { import_run_id: 1 })
        .await
        .unwrap()
}

async fn 利用者(db: &DatabaseConnection, username: &str) -> i32 {
    app_user::ActiveModel {
        name: Set(username.to_owned()),
        username: Set(username.to_owned()),
        email: Set(None),
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
    .id
}

/// プロジェクト、web01、担当（Operator）・承認（Administrator）・部外者を用意する。
///
/// **`username` をそのまま「担当」等にする。**取込が引くのは `username` であり、
/// テストごとにDBが作り直される（`support`）ので一意にする細工は要らない。
async fn 舞台(db: &DatabaseConnection, name: &str) -> 舞台情報 {
    let p = project::ActiveModel {
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
    .unwrap();

    let assignee = 利用者(db, "担当").await;
    let approver = 利用者(db, "承認").await;
    let 外部 = 利用者(db, "部外").await;

    for (user_id, role) in [(assignee, "Operator"), (approver, "Administrator")] {
        project_member::ActiveModel {
            user_id: Set(user_id),
            project_id: Set(p.id),
            role: Set(role.to_owned()),
            admin_rank: Set((role == "Administrator").then(|| "Primary".to_owned())),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(None),
        hostname: Set("web01".to_owned()),
        device_type: Set("Physical".to_owned()),
        power_watt: Set(400),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(p.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(30)),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    舞台情報 {
        project: p,
        device: d,
        assignee,
        approver,
        外部,
    }
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, マイルストーンを登録できる);
        全検証!(@one $用意, $属性, 予定日を動かしても同じ行);
        全検証!(@one $用意, $属性, 識別子の無い行はエラー);
        全検証!(@one $用意, $属性, 語彙外の種別はエラー);
        全検証!(@one $用意, $属性, チケットを登録できる);
        全検証!(@one $用意, $属性, チケットを作ると承認行が起きる);
        全検証!(@one $用意, $属性, 承認を取り込める);
        全検証!(@one $用意, $属性, 承認が揃わない完了は警告);
        全検証!(@one $用意, $属性, 移譲先の規則);
        全検証!(@one $用意, $属性, 中止理由は中止のときだけ);
        全検証!(@one $用意, $属性, メンバー外の担当者は警告);
        全検証!(@one $用意, $属性, 知らない機器はエラー);
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

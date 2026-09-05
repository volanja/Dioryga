//! 共有カタログ画面の結合テスト（設計書16.1のD領域、18章）。
//!
//! **18章が定めた規律が守られているかを確かめる。**
//!
//! - 権限は「いずれか1つ以上のプロジェクトでOperator以上」（18.1）。
//!   **System Adminは触れない**（3章）
//! - 参照されたカタログはスペックを編集できない（18.2）。ただし
//!   **`VENDOR` は対象外**（18.3）
//! - 廃番は参照済みでも設定できる（18.5）
//! - `model_name` に括弧や表記ゆれを持ち込ませない（18.4）

mod support;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, chassis_model, configuration, device, project, project_member, vendor};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, QueryOrder, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 権限（設計書18.1）
// ---------------------------------------------------------------------------

/// **いずれか1つ以上のプロジェクトでOperator以上なら編集できること**（18.1）。
async fn 操作者はカタログを編集できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "op@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(状態, "/catalog/vendors", &token, &[("name", "HPE")]).await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(ベンダー一覧(db).await, vec!["HPE"]);
}

/// **Viewerは編集できないこと**（18.1の「Operator以上」）。
async fn 閲覧者はカタログを編集できない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "viewer@example.com", "Viewer").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(状態, "/catalog/vendors", &token, &[("name", "HPE")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 閲覧はできる。**登録欄が出ないだけ**
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, "/catalog/vendors", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("ベンダーを登録"), "編集欄が出ている");
}

/// **System Adminはカタログに触れないこと**（設計書3章、18.1）。
///
/// 一覧の閲覧すらできない。プロジェクトデータに一切アクセスできないという
/// 制約を、カタログ領域でも維持する。
async fn システム管理者はカタログに入れない(db: &DatabaseConnection) {
    let sa = app_user::ActiveModel {
        name: Set("システム管理者".to_owned()),
        email: Set("sa@example.com".to_owned()),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(true),
        locale: Set("ja".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &sa).await;
    let (status, _) = 取得(状態, "/catalog/vendors", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Vendor（設計書18.3）
// ---------------------------------------------------------------------------

/// **同名のベンダーを2件持てないこと**（18.3）。
///
/// 表記ゆれを防ぐために置いたマスタで同名を持てると、存在意義がなくなる。
async fn 同名のベンダーは登録できない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "dup@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    送信(状態, "/catalog/vendors", &token, &[("name", "HPE")]).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(状態, "/catalog/vendors", &token, &[("name", "HPE")]).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("同じ名前のベンダーが既にあります"));
    assert_eq!(ベンダー一覧(db).await.len(), 1);
}

/// **ベンダー名も正規化されること**（設計書18.3の目的、18.4の規則）。
///
/// `ＨＰＥ` と `HPE` が別行になると、表記ゆれを防ぐために置いたマスタが
/// 表記ゆれの発生源になる。**仮名・漢字は触らない。**
async fn ベンダー名を正規化する(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "vnorm@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    送信(状態, "/catalog/vendors", &token, &[("name", "  ＨＰＥ  ")]).await;
    assert_eq!(ベンダー一覧(db).await, vec!["HPE"]);

    // 正規化の結果として重複するなら弾く
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(状態, "/catalog/vendors", &token, &[("name", "HPE")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("同じ名前のベンダーが既にあります"));

    // 日本語のベンダー名は壊さない
    let (状態, token) = 認証済み(db, &user).await;
    送信(状態, "/catalog/vendors", &token, &[("name", "富士通")]).await;
    assert!(ベンダー一覧(db).await.contains(&"富士通".to_owned()));
}

/// **参照済みのベンダーも改名できること**（設計書18.3）。
///
/// 18.2の「参照されたら編集不可」は `VENDOR` に適用しない。誤字訂正は構成
/// そのものを変えるわけではない。
async fn 参照済みのベンダーは改名できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "rename@example.com", "Operator").await;
    let v = ベンダー(db, "FUJITSU", user.id).await;
    筐体モデル(db, v.id, "PRIMERGY RX2540 M7", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(
        状態,
        "/catalog/vendors",
        &token,
        &[("id", &v.id.to_string()), ("name", "Fujitsu")],
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(ベンダー一覧(db).await, vec!["Fujitsu"]);
}

// ---------------------------------------------------------------------------
// ChassisModel（設計書6.2、18.4）
// ---------------------------------------------------------------------------

/// 登録できること。**自然キーは `(vendor_id, model_name)`**（6.2）。
async fn 筐体モデルを登録できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "model@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 筐体モデルを送る(
        状態,
        &token,
        v.id,
        &[
            ("model_name", "ProLiant DL360 Gen10"),
            ("height_u", "1"),
            ("mount_form", "RackU"),
            ("rack_width", "Full"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let m = 筐体モデル一覧(db).await.pop().unwrap();
    assert_eq!(m.model_name, "ProLiant DL360 Gen10");
    assert_eq!(m.height_u, 1);
    assert_eq!(m.rack_width.as_deref(), Some("Full"));
}

/// **別ベンダーなら同じ製品名を持てること**（設計書6.2）。
async fn 別ベンダーなら同名でも登録できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "sameName@example.com", "Operator").await;
    let a = ベンダー(db, "VendorA", user.id).await;
    let b = ベンダー(db, "VendorB", user.id).await;

    for v in [a.id, b.id] {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, body) = 筐体モデルを送る(
            状態,
            &token,
            v,
            &[
                ("model_name", "Generic 1U"),
                ("height_u", "1"),
                ("mount_form", "RackU"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    }
    assert_eq!(筐体モデル一覧(db).await.len(), 2);

    // 同じベンダーで同じ製品名は弾く
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 筐体モデルを送る(
        状態,
        &token,
        a.id,
        &[
            ("model_name", "Generic 1U"),
            ("height_u", "1"),
            ("mount_form", "RackU"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("同じ製品名の筐体モデルが既にあります"));
}

/// **製品名を正規化すること**（設計書18.4、25.2の段階3）。
///
/// 全角英数を半角に、連続する空白を1つに。18.3でVENDORを作って防いだ表記ゆれを
/// `model_name` で再生産しないため。
async fn 製品名を正規化する(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "normalize@example.com", "Operator").await;
    let v = ベンダー(db, "FUJITSU", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 筐体モデルを送る(
        状態,
        &token,
        v.id,
        &[
            // 全角英数＋全角空白＋連続空白
            ("model_name", "  ＰＲＩＭＥＲＧＹ　RX4770   M8  "),
            ("height_u", "4"),
            ("mount_form", "RackU"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let m = 筐体モデル一覧(db).await.pop().unwrap();
    assert_eq!(m.model_name, "PRIMERGY RX4770 M8");
}

/// **`rack_width` は `RackU` のときだけ意味を持つこと**（設計書6.2）。
///
/// それ以外に値が来たら**黙って捨てず拒否する**（Q-21）。
async fn 幅はrackuでしか指定できない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "width@example.com", "Operator").await;
    let v = ベンダー(db, "APC", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 筐体モデルを送る(
        状態,
        &token,
        v.id,
        &[
            ("model_name", "AP8959 0U PDU"),
            ("height_u", "0"),
            ("mount_form", "RackSide"),
            ("rack_width", "Full"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("RackU の筐体モデルだけ"));
    assert!(筐体モデル一覧(db).await.is_empty());

    // 幅を空にすれば通り、`rack_width` は null になる
    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 筐体モデルを送る(
        状態,
        &token,
        v.id,
        &[
            ("model_name", "AP8959 0U PDU"),
            ("height_u", "0"),
            ("mount_form", "RackSide"),
            ("rack_width", ""),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(筐体モデル一覧(db).await.pop().unwrap().rack_width.is_none());
}

/// 語彙外の搭載形態を拒否すること（Q-21）。
async fn 語彙外の搭載形態は拒否される(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "vocab@example.com", "Operator").await;
    let v = ベンダー(db, "Unknown", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 筐体モデルを送る(
        状態,
        &token,
        v.id,
        &[
            ("model_name", "Mystery Box"),
            ("height_u", "1"),
            ("mount_form", "Wall"),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("搭載形態の値が不正"));
    assert!(筐体モデル一覧(db).await.is_empty());
}

// ---------------------------------------------------------------------------
// 18.2（参照されたら編集不可）
// ---------------------------------------------------------------------------

/// **参照済みの筐体モデルに印が付くこと**（設計書18.2）。
///
/// `CONFIGURATION` と `CHASSIS_SLOT` の両方を見る。
async fn 参照済みの筐体モデルに印が付く(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "referenced@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 参照前) = 取得(状態, "/catalog/chassis-models", &token).await;
    assert!(!参照前.contains("参照済み"));

    構成(db, m.id, "標準構成", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 参照後) = 取得(状態, "/catalog/chassis-models", &token).await;
    assert!(参照後.contains("参照済み"), "印が付いていない");
}

/// **`DEVICE` から参照された構成に印が付くこと**（設計書18.2）。
async fn 参照済みの構成に印が付く(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "confref@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;
    let c = 構成(db, m.id, "標準構成", user.id).await;

    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(Some(c.id)),
        hostname: Set("web01".to_owned()),
        device_type: Set("Physical".to_owned()),
        power_watt: Set(450),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, "/catalog/configurations", &token).await;
    assert!(body.contains("参照済み"));
}

// ---------------------------------------------------------------------------
// 廃番（設計書18.5）
// ---------------------------------------------------------------------------

/// **参照済みでも廃番にできること**（設計書18.5）。
///
/// 18.2が禁じているのは*スペックを定義するフィールド*の編集であり、
/// **選択可否はスペックではない。**
async fn 参照済みでも廃番にできる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "retire@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;
    構成(db, m.id, "標準構成", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(
        状態,
        "/catalog/chassis-models/retire",
        &token,
        &[("id", &m.id.to_string()), ("undo", "0")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 廃番後 = chassis_model::Entity::find_by_id(m.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(廃番後.retired_at.is_some());
}

/// **廃番は一覧の既定で隠れ、切り替えで見えること**（設計書18.5）。
///
/// 既存の参照は壊さない。過去の事実として残る。
async fn 廃番は既定で隠れる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "hidden@example.com", "Operator").await;
    let v = ベンダー(db, "OldVendor", user.id).await;
    vendor::ActiveModel {
        id: Set(v.id),
        retired_at: Set(Some(Utc::now())),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 既定) = 取得(状態, "/catalog/vendors", &token).await;
    assert!(!既定.contains("OldVendor"), "廃番が既定で出ている");

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 全件) = 取得(状態, "/catalog/vendors?retired=1", &token).await;
    assert!(全件.contains("OldVendor"));
}

/// **廃番のベンダーは登録の候補に出ないこと**（設計書18.5）。
///
/// 「以後も選ばれ続ける」ことを止めるのが廃番の目的である。
async fn 廃番は候補に出ない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "candidate@example.com", "Operator").await;
    let 現役 = ベンダー(db, "ActiveVendor", user.id).await;
    let 廃番 = ベンダー(db, "RetiredVendor", user.id).await;
    vendor::ActiveModel {
        id: Set(廃番.id),
        retired_at: Set(Some(Utc::now())),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, "/catalog/chassis-models", &token).await;

    assert!(body.contains(&format!(r#"value="{}""#, 現役.id)));
    assert!(
        !body.contains(&format!(r#"value="{}""#, 廃番.id)),
        "廃番が候補に出ている"
    );
}

/// 廃番を取り消せること。
async fn 廃番を取り消せる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "undo@example.com", "Operator").await;
    let v = ベンダー(db, "BackAgain", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    送信(
        状態,
        "/catalog/vendors/retire",
        &token,
        &[("id", &v.id.to_string()), ("undo", "0")],
    )
    .await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(
        状態,
        "/catalog/vendors/retire",
        &token,
        &[("id", &v.id.to_string()), ("undo", "1")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    assert!(vendor::Entity::find_by_id(v.id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .retired_at
        .is_none());
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// 構成を登録でき、機器登録の選択肢になること。
async fn 構成を登録できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "conf@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(
        状態,
        "/catalog/configurations",
        &token,
        &[
            ("chassis_model_id", &m.id.to_string()),
            ("name", "DL360 高性能Web構成"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let c = configuration::Entity::find().all(db).await.unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].chassis_model_id, m.id);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 筐体モデルを送る(
    state: AppState,
    token: &str,
    vendor_id: i32,
    extra: &[(&str, &str)],
) -> (StatusCode, String) {
    let mut fields: Vec<(&str, String)> = vec![
        ("vendor_id", vendor_id.to_string()),
        ("device_category", "Server".to_owned()),
    ];
    for (k, v) in extra {
        // 同じキーを2つ送らない。上書きする
        if let Some(slot) = fields.iter_mut().find(|(key, _)| key == k) {
            slot.1 = (*v).to_owned();
        } else {
            fields.push((k, (*v).to_owned()));
        }
    }
    let borrowed: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
    送信(state, "/catalog/chassis-models", token, &borrowed).await
}

async fn ベンダー一覧(db: &DatabaseConnection) -> Vec<String> {
    vendor::Entity::find()
        .order_by_asc(vendor::Column::Name)
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.name)
        .collect()
}

async fn 筐体モデル一覧(db: &DatabaseConnection) -> Vec<chassis_model::Model> {
    chassis_model::Entity::find()
        .order_by_asc(chassis_model::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn ベンダー(db: &DatabaseConnection, name: &str, by: i32) -> vendor::Model {
    vendor::ActiveModel {
        name: Set(name.to_owned()),
        retired_at: Set(None),
        created_by: Set(by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 筐体モデル(
    db: &DatabaseConnection,
    vendor_id: i32,
    model_name: &str,
    by: i32,
) -> chassis_model::Model {
    chassis_model::ActiveModel {
        vendor_id: Set(vendor_id),
        model_name: Set(model_name.to_owned()),
        device_category: Set("Server".to_owned()),
        height_u: Set(1),
        mount_form: Set("RackU".to_owned()),
        rack_width: Set(Some("Full".to_owned())),
        retired_at: Set(None),
        created_by: Set(by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 構成(
    db: &DatabaseConnection,
    chassis_model_id: i32,
    name: &str,
    by: i32,
) -> configuration::Model {
    configuration::ActiveModel {
        chassis_model_id: Set(chassis_model_id),
        name: Set(name.to_owned()),
        retired_at: Set(None),
        created_by: Set(by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// カタログ編集はプロジェクトロールで決まる（18.1）。所属だけ作る。
async fn メンバーの利用者(
    db: &DatabaseConnection,
    email: &str,
    role: &str,
) -> app_user::Model {
    let user = 利用者(db, email).await;
    let p = project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(None),
        name: Set(format!("{role}のプロジェクト")),
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

    project_member::ActiveModel {
        user_id: Set(user.id),
        project_id: Set(p.id),
        role: Set(role.to_owned()),
        admin_rank: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    user
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
        name: Set(email.to_owned()),
        email: Set(email.to_owned()),
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

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 操作者はカタログを編集できる);
        全検証!(@one $用意, $属性, 閲覧者はカタログを編集できない);
        全検証!(@one $用意, $属性, システム管理者はカタログに入れない);
        全検証!(@one $用意, $属性, 同名のベンダーは登録できない);
        全検証!(@one $用意, $属性, ベンダー名を正規化する);
        全検証!(@one $用意, $属性, 参照済みのベンダーは改名できる);
        全検証!(@one $用意, $属性, 筐体モデルを登録できる);
        全検証!(@one $用意, $属性, 別ベンダーなら同名でも登録できる);
        全検証!(@one $用意, $属性, 製品名を正規化する);
        全検証!(@one $用意, $属性, 幅はrackuでしか指定できない);
        全検証!(@one $用意, $属性, 語彙外の搭載形態は拒否される);
        全検証!(@one $用意, $属性, 参照済みの筐体モデルに印が付く);
        全検証!(@one $用意, $属性, 参照済みの構成に印が付く);
        全検証!(@one $用意, $属性, 参照済みでも廃番にできる);
        全検証!(@one $用意, $属性, 廃番は既定で隠れる);
        全検証!(@one $用意, $属性, 廃番は候補に出ない);
        全検証!(@one $用意, $属性, 廃番を取り消せる);
        全検証!(@one $用意, $属性, 構成を登録できる);
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

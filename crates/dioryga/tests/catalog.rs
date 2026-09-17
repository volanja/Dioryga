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
use entity::{
    app_user, chassis_model, chassis_slot, configuration, configuration_part, device, part_catalog,
    part_port_slot, port_power_rating, project, project_member, vendor,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
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
        username: Set(("sa@example.com".to_owned()).replace('@', "_")),
        email: Set(Some("sa@example.com".to_owned())),
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

/// **種別は日本語画面で訳して出し、送る値は語彙のままであること**（#126）。
async fn 筐体モデルの種別は表示だけを訳す(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "chassis-label@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 一覧) = 取得(状態, "/catalog/chassis-models", &token).await;
    assert!(一覧.contains("<td>サーバー</td>"), "一覧で訳されていません");
    assert!(一覧.contains(r#"<option value="Server">サーバー</option>"#));
    assert!(一覧.contains(r#"<option value="KVM">KVM</option>"#));

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 詳細) = 取得(状態, &format!("/catalog/chassis-models/{}", m.id), &token).await;
    assert!(詳細.contains("<dd>サーバー</dd>"), "詳細で訳されていません");
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
// 部品カタログ（設計書6.4、8.3、12.7）
// ---------------------------------------------------------------------------

/// **集計に使う値は列、残りはJSON**（設計書6.4のハイブリッド）。
async fn 部品を登録できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "part@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        "/catalog/parts",
        &token,
        &[
            ("vendor_id", &v.id.to_string()),
            ("category", "CPU"),
            ("part_number", "P24479-B21"),
            ("core_count", "16"),
            ("spec_json", r#"{"base_clock_ghz": 2.0}"#),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let p = part_catalog::Entity::find()
        .all(db)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(p.core_count, Some(16));
    assert!(p.capacity_gb.is_none(), "該当しない列が埋まっている");
    assert!(p.spec_json.contains("base_clock_ghz"));
}

/// **壊れたJSONを受け付けないこと**（設計書6.4）。
///
/// 読めない文字列を溜めると、後から使おうとした時点で全件が疑わしくなる。
async fn 壊れたスペックは拒否される(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "badjson@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        "/catalog/parts",
        &token,
        &[
            ("vendor_id", &v.id.to_string()),
            ("category", "Memory"),
            ("part_number", "P07640-B21"),
            ("spec_json", "{壊れている"),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("JSONとして読めません"));
    assert!(part_catalog::Entity::find()
        .all(db)
        .await
        .unwrap()
        .is_empty());
}

/// **`UNIQUE(vendor_id, part_number)`**（設計書18.5）。
async fn 同一ベンダーの同じ型番は登録できない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "partdup@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;

    for _ in 0..2 {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, body) = 送信(
            状態,
            "/catalog/parts",
            &token,
            &[
                ("vendor_id", &v.id.to_string()),
                ("category", "Storage"),
                ("part_number", "P19917-B21"),
            ],
        )
        .await;
        let _ = (status, body);
    }
    assert_eq!(part_catalog::Entity::find().all(db).await.unwrap().len(), 1);
}

/// **`port_speed` は Network、電圧は Power のときだけ**（設計書8.3、12.7）。
///
/// 他の `port_kind` に値が来たら**黙って捨てず拒否する**（Q-21）。
async fn ポートの列は種別ごとに意味を持つ(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "port@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let p = 部品(db, v.id, "NIC", "P08449-B21", user.id).await;

    // Power に速度は指定できない
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ポートを送る(
        状態,
        &token,
        p.id,
        &[
            ("port_kind", "Power"),
            ("port_label", "Inlet"),
            ("connector_type", "IEC C14"),
            ("port_speed", "25G"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Network のポートだけ"));

    // Network に電源定格は付けられない
    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = ポートを送る(
        状態,
        &token,
        p.id,
        &[
            ("port_kind", "Network"),
            ("port_label", "Port1"),
            ("connector_type", "SFP28"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let port = ポート一覧(db, p.id).await.remove(0);
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 定格を送る(状態, &token, p.id, port.id, "AC", "100", "240").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Power のポートだけ"));

    assert!(定格一覧(db, port.id).await.is_empty());
}

/// **電圧は範囲で持つ。片方だけでは意味を成さない**（設計書12.7）。
///
/// 「100-240V対応」と「200V専用」を区別するために範囲にしている。
async fn 電圧は下限と上限の両方が要る(db: &DatabaseConnection) {
    let 場 = 電源ポート(db, "voltage@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 定格を送る(状態, &token, 場.part_id, 場.port_id, "AC", "100", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("下限と上限の両方"));

    // 逆転していても拒否する
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 定格を送る(状態, &token, 場.part_id, 場.port_id, "AC", "240", "100").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("下限が上限を超えています"));

    // 両方揃えば通る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 定格を送る(状態, &token, 場.part_id, 場.port_id, "AC", "100", "240").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let ratings = 定格一覧(db, 場.port_id).await;
    assert_eq!(ratings.len(), 1);
    assert_eq!(ratings[0].current_type, "AC");
    assert_eq!(ratings[0].voltage_min, 100);
    assert_eq!(ratings[0].voltage_max, 240);
}

// ---------------------------------------------------------------------------
// 電源定格（設計書12.7）
// ---------------------------------------------------------------------------

/// **1つのポートが交流と直流の双方を持てること**（設計書12.7）。
///
/// **これが `PORT_POWER_RATING` を子テーブルにした理由そのものである。**
/// Dellには`AC 100~240V`と`DC 240V`の双方を受けるPSUが実在する。列で持つ形では
/// `current_type` を置く場所が無く、範囲も1つしか持てない。
async fn 交流と直流の双方を持てる(db: &DatabaseConnection) {
    let 場 = 電源ポート(db, "acdc@example.com").await;

    for (kind, min, max) in [("AC", "100", "240"), ("DC", "240", "240")] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, _) = 定格を送る(状態, &token, 場.part_id, 場.port_id, kind, min, max).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
    }

    let ratings = 定格一覧(db, 場.port_id).await;
    assert_eq!(ratings.len(), 2, "同じポートに2方式を持てていない");

    // 画面にも両方出る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &format!("/catalog/parts/{}", 場.part_id), &token).await;
    assert!(body.contains("100〜240V"));
    assert!(body.contains("240V"));
}

/// **直流の負電圧は符号を含めたまま扱うこと**（設計書12.7）。
///
/// Ciscoの`-48V`電源は許容範囲が`-40 to -72`であり、**絶対値が大きいほうが
/// 上限ではない。**-72を下限、-40を上限として素直な大小で扱う。
async fn 直流の負電圧は符号込みで扱う(db: &DatabaseConnection) {
    let 場 = 電源ポート(db, "dc@example.com").await;

    // 絶対値で見ていると「-72 > -40」と誤って弾く
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 定格を送る(状態, &token, 場.part_id, 場.port_id, "DC", "-72", "-40").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "負の範囲が拒否された");

    let ratings = 定格一覧(db, 場.port_id).await;
    assert_eq!(ratings[0].voltage_min, -72);
    assert_eq!(ratings[0].voltage_max, -40);
}

/// **方式ごとに1行**（設計書12.7）。同じポートに`AC`を2行持つ意味は無い。
async fn 同じ方式の定格は重ねられない(db: &DatabaseConnection) {
    let 場 = 電源ポート(db, "dup-rating@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 定格を送る(状態, &token, 場.part_id, 場.port_id, "AC", "100", "240").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 定格を送る(状態, &token, 場.part_id, 場.port_id, "AC", "200", "200").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("同じ給電方式の定格が既に"));

    assert_eq!(定格一覧(db, 場.port_id).await.len(), 1);
}

/// 語彙外の給電方式は**既定へ寄せず拒否する**（Q-21）。
async fn 語彙外の給電方式は拒否される(db: &DatabaseConnection) {
    let 場 = 電源ポート(db, "kind@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) =
        定格を送る(状態, &token, 場.part_id, 場.port_id, "AC/DC", "100", "240").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("給電方式の値が不正"));

    assert!(定格一覧(db, 場.port_id).await.is_empty());
}

// ---------------------------------------------------------------------------
// 想定消費電力（設計書12.8）
// ---------------------------------------------------------------------------

/// **3つ揃うか、3つとも空か**（設計書12.8）。
///
/// この3列は「何アンペア引く見込みか」と「PSUの対応範囲に収まっているか」に
/// 答えるためのもので、欠けた組み合わせではどちらにも答えられない。
async fn 想定消費電力は三つ揃うか空か(db: &DatabaseConnection) {
    let 場 = 部品つき構成(db, "power-partial@example.com").await;

    // 電圧だけ
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 電力を送る(状態, &token, 場.configuration_id, "", "200", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("3つとも入れるか"));

    // 3つとも空は「未設定」として通る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 電力を送る(状態, &token, 場.configuration_id, "", "", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let c = 構成を引く(db, 場.configuration_id).await;
    assert!(c.current_type.is_none());
    assert!(c.assumed_voltage.is_none());
    assert!(c.assumed_va.is_none());
}

/// **電流は保存せず計算して見せる**（不変条件2、設計書12.7）。
async fn 想定電流は計算して見せる(db: &DatabaseConnection) {
    let 場 = 部品つき構成(db, "power-amp@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 電力を送る(状態, &token, 場.configuration_id, "AC", "200", "1500").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/catalog/configurations/{}", 場.configuration_id),
        &token,
    )
    .await;
    assert!(body.contains("7.5A"), "VA ÷ V が表示されていない");

    // **0Vは受け付けない。**`A = VA ÷ V` が定義できない
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 電力を送る(状態, &token, 場.configuration_id, "AC", "0", "1500").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("0は指定できません"));
}

/// **DC構成が往復すること**（設計書12.8）。負電圧でも電流は絶対値で出る。
async fn 直流の構成が往復する(db: &DatabaseConnection) {
    let 場 = 部品つき構成(db, "power-dc@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 電力を送る(状態, &token, 場.configuration_id, "DC", "-48", "480").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let c = 構成を引く(db, 場.configuration_id).await;
    assert_eq!(c.current_type.as_deref(), Some("DC"));
    assert_eq!(c.assumed_voltage, Some(-48));
    assert_eq!(c.assumed_va, Some(480));

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/catalog/configurations/{}", 場.configuration_id),
        &token,
    )
    .await;
    assert!(body.contains("10.0A"), "負電圧で電流が出ていない");
}

/// **対応範囲から外れていても保存し、警告する**（設計書12.8、不変条件6）。
///
/// 実機が仕様の想定外であることはありうるし、誤って拒否すると事実を記録できない。
async fn 対応範囲外は保存して警告する(db: &DatabaseConnection) {
    let 場 = 電源つき構成(db, "power-range@example.com", &[("AC", 100, 240)]).await;

    // AC 100〜240V のPSUに、DC -48V を宣言している
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 電力を送る(状態, &token, 場.configuration_id, "DC", "-48", "480").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "警告ではなく拒否している");

    let c = 構成を引く(db, 場.configuration_id).await;
    assert_eq!(c.assumed_voltage, Some(-48), "保存されていない");

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/catalog/configurations/{}", 場.configuration_id),
        &token,
    )
    .await;
    assert!(body.contains("対応する範囲から外れて"));
}

/// **範囲に収まっていれば警告しないこと**（設計書12.8）。
async fn 範囲に収まれば警告しない(db: &DatabaseConnection) {
    // 交流と直流の双方を受けるPSU。**DCで宣言しても通る**
    let 場 = 電源つき構成(
        db,
        "power-ok@example.com",
        &[("AC", 100, 240), ("DC", -72, -40)],
    )
    .await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 電力を送る(状態, &token, 場.configuration_id, "DC", "-48", "480").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/catalog/configurations/{}", 場.configuration_id),
        &token,
    )
    .await;
    assert!(
        !body.contains("対応する範囲から外れて"),
        "-72 ≤ -48 ≤ -40 を絶対値で比べている"
    );
}

/// **定格が1件も登録されていなければ判定しない**（設計書12.8）。
///
/// #53の取込が入るまでデータが無く、「登録されていない」を「違反」と扱うと
/// **警告が常に出る状態になり、それは警告が無いのと同じ**である。
async fn 定格が未登録なら検証しない(db: &DatabaseConnection) {
    let 場 = 電源つき構成(db, "power-none@example.com", &[]).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 電力を送る(状態, &token, 場.configuration_id, "DC", "-48", "480").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/catalog/configurations/{}", 場.configuration_id),
        &token,
    )
    .await;
    assert!(!body.contains("対応する範囲から外れて"));
}

/// 登録フォームからも入れられること（設計書12.8）。
async fn 登録時にも消費電力を入れられる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "power-new@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(
        状態,
        "/catalog/configurations",
        &token,
        &[
            ("chassis_model_id", &m.id.to_string()),
            ("name", "高性能Web構成"),
            ("current_type", "AC"),
            ("assumed_voltage", "200"),
            ("assumed_va", "1500"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let c = configuration::Entity::find()
        .filter(configuration::Column::Name.eq("高性能Web構成"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(c.current_type.as_deref(), Some("AC"));
    assert_eq!(c.assumed_va, Some(1500));
}

// ---------------------------------------------------------------------------
// 構成の部品とスロット（設計書6.1、6.2）
// ---------------------------------------------------------------------------

/// **同じ部品は行を分けず数量で表すこと**（設計書6.2）。
async fn 同じ部品は数量でまとめる(db: &DatabaseConnection) {
    let 場 = 部品つき構成(db, "qty@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 部品を足す(状態, &token, 場.configuration_id, 場.part_id, "8").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 部品を足す(状態, &token, 場.configuration_id, 場.part_id, "4").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に追加されています"));

    let rows = 構成部品(db, 場.configuration_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].quantity, 8);
}

/// **スロット本数の超過は警告に留め、登録は通すこと**（設計書6.1、不変条件6）。
async fn スロット超過は警告に留まる(db: &DatabaseConnection) {
    let 場 = 部品つき構成(db, "slotover@example.com").await;
    // DIMMスロットを2本だけ登録する
    for i in 1..=2 {
        スロット(db, 場.chassis_model_id, "DIMM", &format!("DIMM-{i}")).await;
    }

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 部品を足す(状態, &token, 場.configuration_id, 場.part_id, "8").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("超えています"), "警告が出ていない");
    // **登録は通っている**
    assert_eq!(構成部品(db, 場.configuration_id).await.len(), 1);
}

/// **スロットが1件も無ければ警告を出さないこと**（設計書6.1）。
///
/// 本数が分からないことと、本数が0であることは違う。**警告が常に出る状態は、
/// 警告が無いのと同じである。**
async fn スロット未登録なら警告を出さない(db: &DatabaseConnection) {
    let 場 = 部品つき構成(db, "noslot@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 部品を足す(状態, &token, 場.configuration_id, 場.part_id, "999").await;

    assert_eq!(status, StatusCode::SEE_OTHER, "警告が出てしまっている");
    assert_eq!(構成部品(db, 場.configuration_id).await.len(), 1);
}

/// 本数に収まっていれば警告を出さないこと（設計書6.1）。
async fn 本数に収まれば警告を出さない(db: &DatabaseConnection) {
    let 場 = 部品つき構成(db, "within@example.com").await;
    for i in 1..=8 {
        スロット(db, 場.chassis_model_id, "DIMM", &format!("DIMM-{i}")).await;
    }

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 部品を足す(状態, &token, 場.configuration_id, 場.part_id, "8").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}

/// スロットを登録できること（設計書6.1）。
async fn スロットを登録できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "slot@example.com", "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 送信(
        状態,
        &format!("/catalog/chassis-models/{}/slots", m.id),
        &token,
        &[("slot_type", "DRIVE_BAY"), ("slot_label", "Bay1")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let slots = chassis_slot::Entity::find().all(db).await.unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].slot_type, "DRIVE_BAY");

    // 語彙外は拒否する（Q-21）
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/catalog/chassis-models/{}/slots", m.id),
        &token,
        &[("slot_type", "USB"), ("slot_label", "USB1")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("スロット種別の値が不正"));
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 構成の舞台 {
    user: app_user::Model,
    chassis_model_id: i32,
    configuration_id: i32,
    part_id: i32,
}

async fn 部品つき構成(db: &DatabaseConnection, email: &str) -> 構成の舞台 {
    let user = メンバーの利用者(db, email, "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;
    let c = 構成(db, m.id, "標準構成", user.id).await;
    let p = 部品(db, v.id, "Memory", "P07640-B21", user.id).await;
    構成の舞台 {
        user,
        chassis_model_id: m.id,
        configuration_id: c.id,
        part_id: p.id,
    }
}

async fn 部品を足す(
    state: AppState,
    token: &str,
    configuration_id: i32,
    part_id: i32,
    quantity: &str,
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/catalog/configurations/{configuration_id}/parts"),
        token,
        &[
            ("part_catalog_id", &part_id.to_string()),
            ("quantity", quantity),
        ],
    )
    .await
}

async fn ポートを送る(
    state: AppState,
    token: &str,
    part_id: i32,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/catalog/parts/{part_id}/ports"),
        token,
        fields,
    )
    .await
}

struct 電源の舞台 {
    user: app_user::Model,
    part_id: i32,
    port_id: i32,
}

/// PSUと`port_kind=Power`のポートを1本用意する。
async fn 電源ポート(db: &DatabaseConnection, email: &str) -> 電源の舞台 {
    let user = メンバーの利用者(db, email, "Operator").await;
    let v = ベンダー(db, "Delta", user.id).await;
    let p = 部品(db, v.id, "PSU", "PSU-800W", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = ポートを送る(
        状態,
        &token,
        p.id,
        &[
            ("port_kind", "Power"),
            ("port_label", "Inlet"),
            ("connector_type", "IEC C14"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let port = ポート一覧(db, p.id).await.remove(0);
    電源の舞台 {
        user,
        part_id: p.id,
        port_id: port.id,
    }
}

/// PSUを1つ含む構成。`ratings` に渡した電源定格をそのPSUのポートに付ける。
async fn 電源つき構成(
    db: &DatabaseConnection,
    email: &str,
    ratings: &[(&str, i32, i32)],
) -> 構成の舞台 {
    let user = メンバーの利用者(db, email, "Operator").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let m = 筐体モデル(db, v.id, "DL360 Gen10", user.id).await;
    let c = 構成(db, m.id, "標準構成", user.id).await;
    let psu = 部品(db, v.id, "PSU", "P38995-B21", user.id).await;

    let (状態, token) = 認証済み(db, &user).await;
    部品を足す(状態, &token, c.id, psu.id, "2").await;

    let (状態, token) = 認証済み(db, &user).await;
    ポートを送る(
        状態,
        &token,
        psu.id,
        &[
            ("port_kind", "Power"),
            ("port_label", "Inlet"),
            ("connector_type", "IEC C14"),
        ],
    )
    .await;
    let port = ポート一覧(db, psu.id).await.remove(0);

    for (kind, min, max) in ratings {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 定格を送る(
            状態,
            &token,
            psu.id,
            port.id,
            kind,
            &min.to_string(),
            &max.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
    }

    構成の舞台 {
        user,
        chassis_model_id: m.id,
        configuration_id: c.id,
        part_id: psu.id,
    }
}

async fn 定格を送る(
    state: AppState,
    token: &str,
    part_id: i32,
    port_id: i32,
    current_type: &str,
    min: &str,
    max: &str,
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/catalog/parts/{part_id}/power-ratings"),
        token,
        &[
            ("part_port_slot_id", &port_id.to_string()),
            ("current_type", current_type),
            ("voltage_min", min),
            ("voltage_max", max),
        ],
    )
    .await
}

async fn 電力を送る(
    state: AppState,
    token: &str,
    configuration_id: i32,
    current_type: &str,
    voltage: &str,
    va: &str,
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/catalog/configurations/{configuration_id}/power"),
        token,
        &[
            ("current_type", current_type),
            ("assumed_voltage", voltage),
            ("assumed_va", va),
        ],
    )
    .await
}

async fn 定格一覧(db: &DatabaseConnection, port_id: i32) -> Vec<port_power_rating::Model> {
    port_power_rating::Entity::find()
        .filter(port_power_rating::Column::PartPortSlotId.eq(port_id))
        .order_by_asc(port_power_rating::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 構成を引く(db: &DatabaseConnection, id: i32) -> configuration::Model {
    configuration::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

async fn ポート一覧(db: &DatabaseConnection, part_id: i32) -> Vec<part_port_slot::Model> {
    part_port_slot::Entity::find()
        .filter(part_port_slot::Column::PartCatalogId.eq(part_id))
        .all(db)
        .await
        .unwrap()
}

async fn 構成部品(
    db: &DatabaseConnection,
    configuration_id: i32,
) -> Vec<configuration_part::Model> {
    configuration_part::Entity::find()
        .filter(configuration_part::Column::ConfigurationId.eq(configuration_id))
        .all(db)
        .await
        .unwrap()
}

async fn スロット(
    db: &DatabaseConnection,
    chassis_model_id: i32,
    slot_type: &str,
    label: &str,
) {
    chassis_slot::ActiveModel {
        chassis_model_id: Set(chassis_model_id),
        slot_type: Set(slot_type.to_owned()),
        slot_label: Set(label.to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 部品(
    db: &DatabaseConnection,
    vendor_id: i32,
    category: &str,
    part_number: &str,
    by: i32,
) -> part_catalog::Model {
    part_catalog::ActiveModel {
        category: Set(category.to_owned()),
        vendor_id: Set(vendor_id),
        part_number: Set(part_number.to_owned()),
        core_count: Set(None),
        capacity_gb: Set(None),
        spec_json: Set("{}".to_owned()),
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

// ---------------------------------------------------------------------------
// ダッシュボードとメニュー（#118）
// ---------------------------------------------------------------------------

/// **有効と廃番を並べて数え、統合で吸収された行は数えないこと**（16.1のD領域）。
///
/// 吸収された行は一覧に出さない（23.9.4）ので、数えると一覧の件数と合わない。
async fn ダッシュボードは廃番を分けて数える(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "dash@example.com", "Viewer").await;

    let 残す = ベンダー(db, "HPE", user.id).await;
    let 廃番 = ベンダー(db, "Old Vendor", user.id).await;
    let mut a: vendor::ActiveModel = 廃番.into();
    a.retired_at = Set(Some(Utc::now()));
    a.update(db).await.unwrap();
    let 吸収 = ベンダー(db, "H.P.E.", user.id).await;
    let mut a: vendor::ActiveModel = 吸収.into();
    a.merged_into_vendor_id = Set(Some(残す.id));
    a.merged_at = Set(Some(Utc::now()));
    a.update(db).await.unwrap();

    部品(db, 残す.id, "CPU", "P-1", user.id).await;
    let p = 部品(db, 残す.id, "CPU", "P-2", user.id).await;
    let mut a: part_catalog::ActiveModel = p.into();
    a.retired_at = Set(Some(Utc::now()));
    a.update(db).await.unwrap();

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, "/catalog", &token).await;
    assert_eq!(status, StatusCode::OK);

    // ベンダーは3行あるが、吸収された1行を除いて2件、うち廃番1
    assert!(body.contains(r#"id="count-vendors">2<"#), "{body}");
    assert!(
        body.contains(r#"id="retired-vendors">うち廃番 1<"#),
        "{body}"
    );
    assert!(body.contains(r#"id="count-parts">2<"#), "{body}");
    assert!(body.contains(r#"id="retired-parts">うち廃番 1<"#), "{body}");
    // 登録の無いカタログも0件として並ぶ
    assert!(body.contains(r#"id="count-vlans">0<"#), "{body}");
    // 件数から一覧へ移れる
    assert!(
        body.contains(r#"href="/catalog/parts" id="count-parts""#),
        "{body}"
    );
}

/// **メニューが今いる画面を示すこと**（#118）。
///
/// 詳細画面でも親の一覧の項目に印が付く。下位の項目にいるときは「共有カタログ」
/// に印を付けない（印が2つあるとどちらにいるのか読めない）。
async fn メニューは今いる画面を示す(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "nav@example.com", "Viewer").await;
    let v = ベンダー(db, "HPE", user.id).await;
    let p = 部品(db, v.id, "CPU", "P-1", user.id).await;

    // ダッシュボード：ダッシュボードの項目だけ
    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, "/catalog", &token).await;
    let 左 = 左メニュー(&body);
    assert!(左.contains(r#"href="/catalog" class="current""#), "{左}");
    assert_eq!(左.matches("current").count(), 1, "{左}");
    // 上部は「共有カタログ」に印（#122）
    assert!(
        上部メニュー(&body).contains(r#"href="/catalog" class="current""#),
        "{body}"
    );

    // 一覧：該当する項目だけ。ベンダーがメニューに並ぶ
    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, "/catalog/vendors", &token).await;
    let 左 = 左メニュー(&body);
    assert!(
        左.contains(r#"href="/catalog/vendors" class="current""#),
        "{左}"
    );
    assert_eq!(左.matches("current").count(), 1, "{左}");

    // 詳細：親の一覧の項目
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, &format!("/catalog/parts/{}", p.id), &token).await;
    assert_eq!(status, StatusCode::OK);
    let 左 = 左メニュー(&body);
    assert!(
        左.contains(r#"href="/catalog/parts" class="current""#),
        "{左}"
    );
    assert_eq!(左.matches("current").count(), 1, "{左}");
}

fn 左メニュー(body: &str) -> &str {
    let start = body
        .find(r#"<nav class="sidenav">"#)
        .expect("左メニューがありません");
    let end = start + body[start..].find("</nav>").unwrap();
    &body[start..end]
}

fn 上部メニュー(body: &str) -> &str {
    let start = body
        .find(r#"<nav class="topnav">"#)
        .expect("上部メニューがありません");
    let end = start + body[start..].find("</nav>").unwrap();
    &body[start..end]
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
        全検証!(@one $用意, $属性, 筐体モデルの種別は表示だけを訳す);
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
        全検証!(@one $用意, $属性, 部品を登録できる);
        全検証!(@one $用意, $属性, 壊れたスペックは拒否される);
        全検証!(@one $用意, $属性, 同一ベンダーの同じ型番は登録できない);
        全検証!(@one $用意, $属性, ポートの列は種別ごとに意味を持つ);
        全検証!(@one $用意, $属性, 電圧は下限と上限の両方が要る);
        全検証!(@one $用意, $属性, 交流と直流の双方を持てる);
        全検証!(@one $用意, $属性, 直流の負電圧は符号込みで扱う);
        全検証!(@one $用意, $属性, 同じ方式の定格は重ねられない);
        全検証!(@one $用意, $属性, 語彙外の給電方式は拒否される);
        全検証!(@one $用意, $属性, 想定消費電力は三つ揃うか空か);
        全検証!(@one $用意, $属性, 想定電流は計算して見せる);
        全検証!(@one $用意, $属性, 直流の構成が往復する);
        全検証!(@one $用意, $属性, 対応範囲外は保存して警告する);
        全検証!(@one $用意, $属性, 範囲に収まれば警告しない);
        全検証!(@one $用意, $属性, 定格が未登録なら検証しない);
        全検証!(@one $用意, $属性, 登録時にも消費電力を入れられる);
        全検証!(@one $用意, $属性, 同じ部品は数量でまとめる);
        全検証!(@one $用意, $属性, スロット超過は警告に留まる);
        全検証!(@one $用意, $属性, スロット未登録なら警告を出さない);
        全検証!(@one $用意, $属性, 本数に収まれば警告を出さない);
        全検証!(@one $用意, $属性, スロットを登録できる);
        全検証!(@one $用意, $属性, ダッシュボードは廃番を分けて数える);
        全検証!(@one $用意, $属性, メニューは今いる画面を示す);
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

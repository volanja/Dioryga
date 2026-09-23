//! 残りのカタログ画面の結合テスト（設計書16.1のD領域、8.7・9.4・8.5）。
//!
//! **#68 が扱う3画面**——ケーブル・ソフトウェア・VLAN。それぞれ守るべき規律が
//! 違い、いずれも「素直に実装すると壊す」点を持っている。
//!
//! | 画面 | 壊しやすい点 |
//! |---|---|
//! | ケーブル | 端を2本に縛る／定格をネットワークにも出す／メートルとミリの取り違え |
//! | ソフトウェア | `purl` を空文字で保存し、2件目から登録できなくなる |
//! | VLAN | タグを一意にし、複数拠点を1つの台帳で扱えなくする |

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, cable_catalog, cable_end_slot, project, project_member, software_catalog, vlan,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// ケーブル（設計書8.7）
// ---------------------------------------------------------------------------

/// **長さはメートルで受け、ミリメートルの整数で保存すること**（設計書24.2.1）。
///
/// SQLiteに `DECIMAL` が無いため整数で持つ。ここを取り違えると1000倍ずれる。
async fn 長さはミリメートルで保存する(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "cable-len@example.com").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ケーブルを送る(
        状態,
        &token,
        &[
            ("cable_kind", "Network"),
            ("cable_type", "Cat6A"),
            ("length_m", "1.8"),
            ("color", "blue"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let c = ケーブル一覧(db).await.remove(0);
    assert_eq!(c.length_mm, Some(1800));
    assert_eq!(c.cable_kind.as_deref(), Some("Network"));
}

/// **定格を指定できるのは電源ケーブルだけであること**（設計書8.7）。
///
/// 選択肢はサーバが描いているため、ここに値が来るのは改竄しかない（8.6）。
async fn 定格はネットワークには付けられない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "cable-rating@example.com").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ケーブルを送る(
        状態,
        &token,
        &[
            ("cable_kind", "Network"),
            ("cable_type", "OM4"),
            ("rated_voltage", "250"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("電源ケーブルだけ"));
    assert!(ケーブル一覧(db).await.is_empty());
}

/// **定格電流はアンペアで受け、mAの整数で保存すること**（設計書24.2.1）。
async fn 定格電流はミリアンペアで保存する(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "cable-ma@example.com").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = ケーブルを送る(
        状態,
        &token,
        &[
            ("cable_kind", "Power"),
            ("cable_type", "Power Cord"),
            ("rated_voltage", "250"),
            ("rated_current_a", "12"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let c = ケーブル一覧(db).await.remove(0);
    assert_eq!(c.rated_current_ma, Some(12000));
    assert_eq!(c.rated_voltage, Some(250));

    // 一覧では A に戻して見せる
    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, "/catalog/cables?kind=Power", &token).await;
    assert!(body.contains("12.0A"), "アンペアで表示されていない");
    assert!(body.contains("250V"));
}

/// **種別で一覧とフォームが分かれること**（設計書8.7）。
///
/// テーブルは1つのまま、入力の手間だけを解消するのがこの分け方の狙いである。
async fn 種別で一覧と入力欄が分かれる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "cable-kind@example.com").await;

    // **ヒント文にも "Power Cord" が出るため、判別できる名前を使う**
    for (kind, cable_type) in [("Network", "OM4-LC-LC"), ("Power", "C13-NEMA")] {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = ケーブルを送る(
            状態,
            &token,
            &[("cable_kind", kind), ("cable_type", cable_type)],
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
    }

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, "/catalog/cables?kind=Network", &token).await;
    assert!(body.contains("OM4-LC-LC"));
    assert!(!body.contains("C13-NEMA"), "電源が混ざっている");

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, "/catalog/cables?kind=Power", &token).await;
    assert!(body.contains("C13-NEMA"));
    assert!(!body.contains("OM4-LC-LC"), "ネットワークが混ざっている");

    // **登録画面は種別を引き継ぎ、定格の欄は電源のときだけ出す**（#124）
    let (状態, token) = 認証済み(db, &user).await;
    let (_, 光) = 取得(状態, "/catalog/cables/new?kind=Network", &token).await;
    assert!(!光.contains("定格電流"), "定格の欄が出ている: {光}");

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 電源) = 取得(状態, "/catalog/cables/new?kind=Power", &token).await;
    assert!(電源.contains("定格電流"), "{電源}");
}

/// **端ごとに異なるコネクタを持てること**（設計書8.7）。
///
/// NEMA 5-15P と C13 のような非対称なケーブルが実在する。**端を2本に縛らない**
/// ——MPOブレイクアウトは Trunk 1本と Branch 4本を持つ。
async fn 端ごとに異なるコネクタを持てる(db: &DatabaseConnection) {
    let 場 = ケーブル(db, "cable-ends@example.com", "Power", "Power Cord").await;

    for (label, connector) in [("A", "NEMA 5-15P"), ("B", "IEC C13")] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, _) = 端を送る(状態, &token, 場.cable_id, label, connector, "").await;
        assert_eq!(status, StatusCode::SEE_OTHER);
    }

    let ends = 端一覧(db, 場.cable_id).await;
    assert_eq!(ends.len(), 2);
    assert_eq!(ends[0].connector_type, "NEMA 5-15P");
    assert_eq!(ends[1].connector_type, "IEC C13");

    // **3本以上も持てる**（ブレイクアウト）
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 端を送る(状態, &token, 場.cable_id, "Branch1", "LC", "10G").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "端が2本に縛られている");
    assert_eq!(端一覧(db, 場.cable_id).await.len(), 3);
}

/// 同じラベルの端は重複させないこと。
async fn 同じ端のラベルは重複できない(db: &DatabaseConnection) {
    let 場 = ケーブル(db, "cable-dup@example.com", "Network", "Cat6A").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    端を送る(状態, &token, 場.cable_id, "A", "RJ-45", "").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 端を送る(状態, &token, 場.cable_id, "A", "LC", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に登録されています"));
    assert_eq!(端一覧(db, 場.cable_id).await.len(), 1);
}

// ---------------------------------------------------------------------------
// ソフトウェアカタログ（設計書9.4）
// ---------------------------------------------------------------------------

/// **`purl` を持たない行は何件でも共存できること**（設計書9.4）。
///
/// `purl` はUNIQUEだが nullable であり、両DBともNULL同士は重複と見なさない。
/// **空文字で保存すると2件目から登録できなくなる。**
async fn purlなしを何件でも登録できる(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "sw-purl@example.com").await;

    for name in ["社内ツールA", "社内ツールB"] {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, body) = ソフトウェアを送る(
            状態,
            &token,
            &[
                ("name", name),
                ("version", "1.0"),
                ("category", "Application"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{name}: {body}");
    }

    let list = ソフトウェア一覧(db).await;
    assert_eq!(list.len(), 2);
    // **空文字ではなくNULLで保存されていること**
    assert!(list.iter().all(|s| s.purl.is_none()));
}

/// `purl` があるときは重複を拒否すること（設計書9.4）。
async fn 同じpurlは登録できない(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "sw-dup@example.com").await;
    let 送る = |状態, token: String| async move {
        ソフトウェアを送る(
            状態,
            &token,
            &[
                ("name", "log4j-core"),
                ("version", "2.17.1"),
                ("category", "Library"),
                (
                    "purl",
                    "pkg:maven/org.apache.logging.log4j/log4j-core@2.17.1",
                ),
            ],
        )
        .await
    };

    let (状態, token) = 認証済み(db, &user).await;
    assert_eq!(送る(状態, token).await.0, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送る(状態, token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に登録されています"));
    assert_eq!(ソフトウェア一覧(db).await.len(), 1);
}

/// 語彙外の種別は**既定へ寄せず拒否すること**（設計書8.6、Q-21）。
async fn 語彙外のソフトウェア種別は拒否される(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "sw-kind@example.com").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = ソフトウェアを送る(
        状態,
        &token,
        &[
            ("name", "nginx"),
            ("version", "1.24.0"),
            ("category", "ミドルウェア"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("種別の値が不正"));
    assert!(ソフトウェア一覧(db).await.is_empty());
}

// ---------------------------------------------------------------------------
// VLAN（設計書8.3、8.5）
// ---------------------------------------------------------------------------

/// **同じタグを複数登録できること**（設計書8.3）。
///
/// VLANタグはL2ドメインごとに独立しており、**拠点が違えば同じ `VLAN 100` が
/// 別物として存在する。**一意にすると複数拠点を1つの台帳で扱えなくなる。
///
/// **ただし取り違えが起きやすいので警告する**（不変条件6）。
async fn 同じタグを登録できるが警告する(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "vlan-dup@example.com").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = vlanを送る(状態, &token, &[("vlan_tag", "100"), ("name", "東京-業務")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) =
        vlanを送る(状態, &token, &[("vlan_tag", "100"), ("name", "大阪-業務")]).await;

    // **拒否ではない。**登録したうえで警告を見せる
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に使われています"));
    assert_eq!(vlan一覧(db).await.len(), 2, "同じタグが登録できていない");
}

/// **802.1Qの範囲外は拒否すること。**0と4095は予約されている。
async fn 範囲外のタグは拒否される(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "vlan-range@example.com").await;

    for tag in ["0", "4095", "-1", "abc"] {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, body) = vlanを送る(状態, &token, &[("vlan_tag", tag), ("name", "N")]).await;
        assert_eq!(status, StatusCode::OK, "{tag} が通ってしまった");
        assert!(body.contains("1〜4094"), "{tag}");
    }
    assert!(vlan一覧(db).await.is_empty());

    // 境界は通る
    for tag in ["1", "4094"] {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = vlanを送る(状態, &token, &[("vlan_tag", tag), ("name", "N")]).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{tag} が拒否された");
    }
}

/// 語彙外のゾーンは拒否し、未設定は許すこと（設計書8.5、Q-21）。
async fn ゾーンは語彙に従う(db: &DatabaseConnection) {
    let user = メンバーの利用者(db, "vlan-zone@example.com").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = vlanを送る(
        状態,
        &token,
        &[("vlan_tag", "200"), ("name", "N"), ("zone", "外部")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ゾーンの値が不正"));

    // 未設定は許す
    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = vlanを送る(状態, &token, &[("vlan_tag", "200"), ("name", "N")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(vlan一覧(db).await[0].zone.is_none());
}

// ---------------------------------------------------------------------------
// 権限（設計書18.1）
// ---------------------------------------------------------------------------

/// **Viewerは編集できないこと。**閲覧はできる（18.1）。
async fn 閲覧者は残りのカタログを編集できない(db: &DatabaseConnection) {
    let user = 役つき利用者(db, "rest-viewer@example.com", "Viewer").await;

    for path in ["/catalog/cables", "/catalog/software", "/catalog/vlans"] {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 送信(状態, path, &token, &[("name", "x")]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");

        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 取得(状態, path, &token).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct ケーブルの舞台 {
    user: app_user::Model,
    cable_id: i32,
}

async fn ケーブル(
    db: &DatabaseConnection,
    email: &str,
    kind: &str,
    cable_type: &str,
) -> ケーブルの舞台 {
    let user = メンバーの利用者(db, email).await;
    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = ケーブルを送る(
        状態,
        &token,
        &[("cable_kind", kind), ("cable_type", cable_type)],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let cable_id = ケーブル一覧(db).await.remove(0).id;
    ケーブルの舞台 { user, cable_id }
}

async fn ケーブルを送る(
    state: AppState,
    token: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(state, "/catalog/cables", token, fields).await
}

async fn 端を送る(
    state: AppState,
    token: &str,
    cable_id: i32,
    label: &str,
    connector: &str,
    speed: &str,
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/catalog/cables/{cable_id}/ends"),
        token,
        &[
            ("end_label", label),
            ("connector_type", connector),
            ("port_speed", speed),
        ],
    )
    .await
}

async fn ソフトウェアを送る(
    state: AppState,
    token: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(state, "/catalog/software", token, fields).await
}

async fn vlanを送る(
    state: AppState,
    token: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(state, "/catalog/vlans", token, fields).await
}

async fn ケーブル一覧(db: &DatabaseConnection) -> Vec<cable_catalog::Model> {
    cable_catalog::Entity::find()
        .order_by_asc(cable_catalog::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 端一覧(db: &DatabaseConnection, cable_id: i32) -> Vec<cable_end_slot::Model> {
    cable_end_slot::Entity::find()
        .filter(cable_end_slot::Column::CableCatalogId.eq(cable_id))
        .order_by_asc(cable_end_slot::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn ソフトウェア一覧(db: &DatabaseConnection) -> Vec<software_catalog::Model> {
    software_catalog::Entity::find().all(db).await.unwrap()
}

async fn vlan一覧(db: &DatabaseConnection) -> Vec<vlan::Model> {
    vlan::Entity::find()
        .order_by_asc(vlan::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn メンバーの利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    役つき利用者(db, email, "Operator").await
}

/// カタログ編集はプロジェクトロールで決まる（18.1）。所属だけ作る。
async fn 役つき利用者(db: &DatabaseConnection, email: &str, role: &str) -> app_user::Model {
    let user = 利用者(db, email).await;
    let p = project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(None),
        name: Set(format!("P-{email}")),
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
        project_id: Set(p.id),
        user_id: Set(user.id),
        role: Set(role.to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    user
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("カタログ担当".to_owned()),
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(false),
        locale: Set("ja".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
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

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 長さはミリメートルで保存する);
        全検証!(@one $用意, $属性, 定格はネットワークには付けられない);
        全検証!(@one $用意, $属性, 定格電流はミリアンペアで保存する);
        全検証!(@one $用意, $属性, 種別で一覧と入力欄が分かれる);
        全検証!(@one $用意, $属性, 端ごとに異なるコネクタを持てる);
        全検証!(@one $用意, $属性, 同じ端のラベルは重複できない);
        全検証!(@one $用意, $属性, purlなしを何件でも登録できる);
        全検証!(@one $用意, $属性, 同じpurlは登録できない);
        全検証!(@one $用意, $属性, 語彙外のソフトウェア種別は拒否される);
        全検証!(@one $用意, $属性, 同じタグを登録できるが警告する);
        全検証!(@one $用意, $属性, 範囲外のタグは拒否される);
        全検証!(@one $用意, $属性, ゾーンは語彙に従う);
        全検証!(@one $用意, $属性, 閲覧者は残りのカタログを編集できない);
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

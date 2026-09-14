//! ソフトウェア・SBOMのスキーマの結合テスト（設計書9章）。

mod support;

use chrono::Utc;
use entity::{
    app_user, device, sbom_component_change, sbom_component_index, sbom_import, sbom_snapshot,
    software_catalog, software_installation, software_instance, software_role_assignment, vendor,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter,
    Set, Statement,
};

/// 9.8の検索結果。生SQLをそのまま流すために用意する。
#[derive(Debug, FromQueryResult)]
struct 該当機器 {
    #[allow(dead_code)]
    hostname: String,
    version: String,
}

/// バージョンアップが「閉じて開く」で表せること（9.3）。
///
/// **ライセンスキーは新しいインスタンスへ引き継ぐ。**バージョンアップを跨いで
/// 引き継がれる固有情報こそが `SOFTWARE_INSTANCE` の存在理由である（9.3）。
async fn バージョンアップは履歴として残る(db: &DatabaseConnection) {
    let 端末 = 機器(db, "app-01").await;
    let 旧 = カタログ(db, "PostgreSQL", "16.2").await;
    let 新 = カタログ(db, "PostgreSQL", "17.0").await;

    let 旧実体 = インスタンス(db, 旧.id, Some("LIC-0001")).await;
    let 旧設置 = 設置(db, 旧実体.id, 端末.id).await;

    // 旧行を閉じ、新しいインスタンスの行を開く
    閉じる(db, 旧設置).await;
    let 新実体 = インスタンス(db, 新.id, Some("LIC-0001")).await;
    設置(db, 新実体.id, 端末.id).await;

    let 履歴 = software_installation::Entity::find()
        .filter(software_installation::Column::DeviceId.eq(端末.id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(履歴.len(), 2, "旧行が上書きされている");

    let 現行: Vec<_> = 履歴.iter().filter(|r| r.to_date.is_none()).collect();
    assert_eq!(現行.len(), 1);
    assert_eq!(現行[0].software_instance_id, 新実体.id);
}

/// 同じインスタンスを2台へ同時に入れられないこと。
///
/// ライセンスの実体は1つであり、移設は「閉じて開く」で表す（不変条件1）。
async fn 同一インスタンスは同時に一台だけ(db: &DatabaseConnection) {
    let a = 機器(db, "dup-a").await;
    let b = 機器(db, "dup-b").await;
    let cat = カタログ(db, "Oracle Database", "19c").await;
    let 実体 = インスタンス(db, cat.id, Some("LIC-DUP")).await;

    設置(db, 実体.id, a.id).await;

    let 二台目 = software_installation::ActiveModel {
        software_instance_id: Set(実体.id),
        device_id: Set(b.id),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await;
    assert!(二台目.is_err(), "同じライセンスが2台に入ってしまった");
}

/// 1つのインストールに複数のroleを付与できること（9.9）。
///
/// dnsmasqのようにDNSとDHCPを兼ねるミドルウェアを表すため。
/// **ただし同じroleは二重に付かない。**
async fn 複数のロールを兼務できる(db: &DatabaseConnection) {
    let d = 機器(db, "infra-01").await;
    let cat = カタログ(db, "dnsmasq", "2.90").await;
    let 実体 = インスタンス(db, cat.id, None).await;
    let 設置id = 設置(db, 実体.id, d.id).await;

    for role in ["DNS", "DHCP"] {
        ロール(db, 設置id, role).await.unwrap();
    }

    let 一覧 = software_role_assignment::Entity::find()
        .filter(software_role_assignment::Column::SoftwareInstallationId.eq(設置id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(一覧.len(), 2);

    assert!(
        ロール(db, 設置id, "DNS").await.is_err(),
        "同じroleが二重に付いてしまった"
    );
}

/// **内容アドレス指定が効くこと**（9.7）。
///
/// ゴールデンイメージから構築された2台のSBOMは完全に一致する。同一内容の
/// スナップショットは1件しか保存されず、**保存量が台数に比例しない。**
async fn 同一内容のスナップショットは一件だけ(db: &DatabaseConnection) {
    let 利用者 = 利用者(db, "sbom@example.com").await;
    let a = 機器(db, "golden-a").await;
    let b = 機器(db, "golden-b").await;

    // 同じイメージなので内容も同じ。ハッシュが一致する
    let hash = "aa".repeat(32);
    スナップショット(db, &hash, 3000).await;

    for d in [a.id, b.id] {
        取込(db, d, &hash, 利用者.id).await;
    }

    let 一覧 = sbom_snapshot::Entity::find().all(db).await.unwrap();
    assert_eq!(一覧.len(), 1, "同一内容が2件保存されている");
    assert_eq!(一覧[0].component_count, 3000);

    // 取込の記録は台数ぶんある。共有されているのはスナップショットだけ
    let 取込一覧 = sbom_import::Entity::find().all(db).await.unwrap();
    assert_eq!(取込一覧.len(), 2);
}

/// zstd圧縮した内容が往復し、**実際に一桁縮むこと**（9.7）。
///
/// blobを両DBで同じように扱えることの確認を兼ねる。**中身の形式はJSONのまま**
/// とし、デバッグ容易性を優先している。
///
/// 9.7は「SBOMは同じパッケージ名・バージョン文字列が大量に反復する冗長性の
/// 高いデータなので10〜20倍のオーダーで削減できる」と主張している。**1件だけ
/// のJSONではヘッダ分で逆に増える**ため、現実に近い件数で確かめる。
async fn 圧縮した内容が往復する(db: &DatabaseConnection) {
    let 元 = 現実的なSBOM(3000);
    let 圧縮 = zstd::encode_all(&元[..], 3).unwrap();
    let 比 = 元.len() as f64 / 圧縮.len() as f64;
    assert!(
        比 > 10.0,
        "9.7が見込んだ10倍に届いていない（{:.1}倍、{} → {}バイト）",
        比,
        元.len(),
        圧縮.len()
    );

    let hash = "bb".repeat(32);
    sbom_snapshot::ActiveModel {
        content_hash: Set(hash.clone()),
        content: Set(圧縮),
        component_count: Set(3000),
        first_seen_at: Set(Utc::now()),
    }
    .insert(db)
    .await
    .unwrap();

    let 取得 = sbom_snapshot::Entity::find_by_id(hash)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let 展開 = zstd::decode_all(&取得.content[..]).unwrap();
    assert_eq!(展開, 元);
}

/// 1台につき最新の観測は1件だけであること（9.5）。
///
/// `superseded_at` は他の履歴テーブルの `to_date` と同じ役割を果たす。
async fn 最新の観測は一台に一件(db: &DatabaseConnection) {
    let 利用者 = 利用者(db, "supersede@example.com").await;
    let d = 機器(db, "sup-01").await;
    let 旧hash = "c1".repeat(32);
    let 新hash = "c2".repeat(32);
    スナップショット(db, &旧hash, 10).await;
    スナップショット(db, &新hash, 12).await;

    let 旧取込 = 取込(db, d.id, &旧hash, 利用者.id).await;

    // 閉じる前に新しい取込を作ろうとすると弾かれる
    assert!(
        sbom_import::ActiveModel {
            device_id: Set(d.id),
            content_hash: Set(新hash.clone()),
            source_format: Set("CycloneDX".to_owned()),
            work_order_id: Set(None),
            imported_by: Set(利用者.id),
            imported_at: Set(Utc::now()),
            superseded_at: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await
        .is_err(),
        "最新の観測が1台に2件できてしまった"
    );

    sbom_import::ActiveModel {
        id: Set(旧取込),
        superseded_at: Set(Some(Utc::now())),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let 新取込 = 取込(db, d.id, &新hash, 利用者.id).await;

    // added が2件、version_changed が1件という差分を残す
    for (種別, from, to) in [
        ("added", None, Some("1.0.0")),
        ("added", None, Some("2.0.0")),
        ("version_changed", Some("3.0.12"), Some("3.0.13")),
    ] {
        sbom_component_change::ActiveModel {
            sbom_import_id: Set(新取込),
            change_type: Set(種別.to_owned()),
            name: Set("openssl".to_owned()),
            purl: Set(None),
            version_from: Set(from.map(str::to_owned)),
            version_to: Set(to.map(str::to_owned)),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    let 差分 = sbom_component_change::Entity::find()
        .filter(sbom_component_change::Column::SbomImportId.eq(新取込))
        .all(db)
        .await
        .unwrap();
    assert_eq!(差分.len(), 3);
}

/// 9.8の横断検索が引けること。
///
/// 「特定のバージョンのライブラリが入っている機器を全て挙げる」。
/// **索引は `content_hash` 単位に張っているため、機器が何台あっても索引は
/// 増えない。**
async fn 横断検索で機器を引ける(db: &DatabaseConnection) {
    let 利用者 = 利用者(db, "search@example.com").await;
    let hash = "dd".repeat(32);
    スナップショット(db, &hash, 2).await;

    for (purl, name, version) in [
        (
            Some("pkg:maven/org.apache.logging.log4j/log4j-core@2.14.1"),
            "log4j-core",
            "2.14.1",
        ),
        (None, "custom-agent", "1.2.3"),
    ] {
        sbom_component_index::ActiveModel {
            content_hash: Set(hash.clone()),
            purl: Set(purl.map(str::to_owned)),
            name: Set(name.to_owned()),
            version: Set(version.to_owned()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    // 同じイメージの機器を3台。索引は1組しか無い
    for i in 0..3 {
        let d = 機器(db, &format!("search-{i}")).await;
        取込(db, d.id, &hash, 利用者.id).await;
    }

    // **設計書9.8が載せているSQLをそのまま流す。**「両DB共通のSQLで書ける」と
    // 主張している以上、そのまま動くことを確かめる価値がある
    let 該当 = 該当機器::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        r#"
            SELECT d.hostname, i.version
            FROM sbom_import si
            JOIN sbom_component_index i ON i.content_hash = si.content_hash
            JOIN device d ON d.id = si.device_id
            WHERE si.superseded_at IS NULL
              AND i.purl LIKE $1
            "#,
        ["pkg:maven/org.apache.logging.log4j/log4j-core@%".into()],
    ))
    .all(db)
    .await
    .unwrap();
    assert_eq!(該当.len(), 3, "3台とも引けること");
    assert!(該当.iter().all(|r| r.version == "2.14.1"));

    let 索引 = sbom_component_index::Entity::find().all(db).await.unwrap();
    assert_eq!(索引.len(), 2, "索引が機器の台数ぶんに増えている");

    // purl を持たないコンポーネントは name で引く（9.6）
    let name検索 = sbom_component_index::Entity::find()
        .filter(sbom_component_index::Column::Name.eq("custom-agent"))
        .all(db)
        .await
        .unwrap();
    assert_eq!(name検索.len(), 1);
    assert!(name検索[0].purl.is_none());
}

/// 未インストールのライセンス在庫と失効済みが区別できること（9.4.1）。
///
/// どちらも現行の設置行を持たない。**`retired_at` だけがこの2つを分ける。**
async fn 在庫と失効を区別できる(db: &DatabaseConnection) {
    let cat = カタログ(db, "Visio", "2021").await;
    let 在庫 = インスタンス(db, cat.id, Some("LIC-STOCK")).await;
    let 失効 = インスタンス(db, cat.id, Some("LIC-DEAD")).await;

    software_instance::ActiveModel {
        id: Set(失効.id),
        retired_at: Set(Some(Utc::now())),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    // どちらも設置行は無い
    for id in [在庫.id, 失効.id] {
        let 設置 = software_installation::Entity::find()
            .filter(software_installation::Column::SoftwareInstanceId.eq(id))
            .all(db)
            .await
            .unwrap();
        assert!(設置.is_empty());
    }

    let 保有中 = software_instance::Entity::find()
        .filter(software_instance::Column::RetiredAt.is_null())
        .filter(software_instance::Column::SoftwareCatalogId.eq(cat.id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(保有中.len(), 1);
    assert_eq!(保有中[0].id, 在庫.id);
}

// --- 補助 -----------------------------------------------------------------

/// 現実に近いSBOM。**同じ文字列が大量に反復する**という9.7の前提を再現する。
#[allow(non_snake_case)]
fn 現実的なSBOM(件数: usize) -> Vec<u8> {
    let mut s = String::from("[");
    for i in 0..件数 {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            r#"{{"name":"lib-{i}","version":"1.{}.0","purl":"pkg:maven/org.example/lib-{i}@1.{}.0","license_expression":"Apache-2.0"}}"#,
            i % 20,
            i % 20
        ));
    }
    s.push(']');
    s.into_bytes()
}

async fn スナップショット(db: &DatabaseConnection, hash: &str, count: i32) {
    sbom_snapshot::ActiveModel {
        content_hash: Set(hash.to_owned()),
        content: Set(zstd::encode_all(&b"[]"[..], 3).unwrap()),
        component_count: Set(count),
        first_seen_at: Set(Utc::now()),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 取込(db: &DatabaseConnection, device_id: i32, hash: &str, by: i32) -> i32 {
    sbom_import::ActiveModel {
        device_id: Set(device_id),
        content_hash: Set(hash.to_owned()),
        source_format: Set("CycloneDX".to_owned()),
        work_order_id: Set(None),
        imported_by: Set(by),
        imported_at: Set(Utc::now()),
        superseded_at: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

async fn ロール(
    db: &DatabaseConnection,
    installation_id: i32,
    role: &str,
) -> Result<software_role_assignment::Model, sea_orm::DbErr> {
    software_role_assignment::ActiveModel {
        software_installation_id: Set(installation_id),
        role: Set(role.to_owned()),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
}

async fn 閉じる(db: &DatabaseConnection, installation_id: i32) {
    software_installation::ActiveModel {
        id: Set(installation_id),
        to_date: Set(Some(Utc::now())),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

async fn 設置(db: &DatabaseConnection, instance_id: i32, device_id: i32) -> i32 {
    software_installation::ActiveModel {
        software_instance_id: Set(instance_id),
        device_id: Set(device_id),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

async fn インスタンス(
    db: &DatabaseConnection,
    catalog_id: i32,
    key: Option<&str>,
) -> software_instance::Model {
    software_instance::ActiveModel {
        software_catalog_id: Set(catalog_id),
        license_key: Set(key.map(str::to_owned)),
        asset_number: Set(None),
        retired_at: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn カタログ(
    db: &DatabaseConnection,
    name: &str,
    version: &str,
) -> software_catalog::Model {
    let v = vendor::ActiveModel {
        name: Set(format!("{name}社-{version}")),
        created_by: Set(登録者(db).await),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    software_catalog::ActiveModel {
        name: Set(name.to_owned()),
        vendor_id: Set(Some(v.id)),
        version: Set(version.to_owned()),
        category: Set("Application".to_owned()),
        purl: Set(None),
        license_expression: Set(String::new()),
        spec_json: Set("{}".to_owned()),
        created_by: Set(登録者(db).await),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// 登録者は使い回す。テストの主題ではない。
async fn 登録者(db: &DatabaseConnection) -> i32 {
    if let Some(u) = app_user::Entity::find()
        .filter(app_user::Column::Email.eq("owner@example.com"))
        .one(db)
        .await
        .unwrap()
    {
        return u.id;
    }
    利用者(db, "owner@example.com").await.id
}

async fn 機器(db: &DatabaseConnection, hostname: &str) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        hostname: Set(hostname.to_owned()),
        device_type: Set("Physical".to_owned()),
        power_watt: Set(350),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        name: Set(email.to_owned()),
        password_hash: Set("x".to_owned()),
        is_system_admin: Set(false),
        must_change_password: Set(false),
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
        全検証!(@one $用意, $属性, バージョンアップは履歴として残る);
        全検証!(@one $用意, $属性, 同一インスタンスは同時に一台だけ);
        全検証!(@one $用意, $属性, 複数のロールを兼務できる);
        全検証!(@one $用意, $属性, 同一内容のスナップショットは一件だけ);
        全検証!(@one $用意, $属性, 圧縮した内容が往復する);
        全検証!(@one $用意, $属性, 最新の観測は一台に一件);
        全検証!(@one $用意, $属性, 横断検索で機器を引ける);
        全検証!(@one $用意, $属性, 在庫と失効を区別できる);
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

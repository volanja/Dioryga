//! 機器の詳細の「部品」の節の結合テスト（設計書6.3、16.1、#255）。
//!
//! 「ポートとIP」の節は `network.rs` が確かめる。ここでは次を押さえる。
//!
//! - 表の行は構成から描くだけで、台帳に載るのは人が「載せる」を保存したとき
//! - シリアル番号は無くても載せられ、あとから同じ画面で入れられる
//! - 交換はシリアル番号の書き換えで、インターフェースはそのまま残る
//! - 倉庫プロジェクトの予備品を使うと、予備品の所在が `Used` になる

use axum::http::StatusCode;
use chrono::Utc;
use entity::{
    audit_log, configuration_part, device, part_catalog, part_instance, part_instance_location,
    part_port_slot,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use super::network::{
    インターフェース, プロジェクト, 取得, 現在のip, 節を送る, 舞台, 舞台の役つき, 舞台情報, 詳細,
    認証済み, 送信,
};

/// **構成の部品が、台帳に載っている・いないの区別つきで並ぶこと。**
/// 構成を開いただけでは `PART_INSTANCE` を作らない（6.3）。
async fn 構成の部品が並ぶ(db: &DatabaseConnection) {
    let 場 = 舞台(db, "sec-list@example.com").await;
    let nic = 載っている型(db, &場).await;
    let mem = 型を作る(db, &場, "Memory", "DIMM-32G").await;
    構成に足す(db, &場, nic, 2).await;
    構成に足す(db, &場, mem, 16).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &詳細(&場), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("DIMM-32G"));
    // NICは2枚のうち1枚が載っている。メモリは載っていない
    assert!(body.contains(&format!(r#"name="register_n{nic}_0""#)));
    assert!(!body.contains(&format!(r#"name="register_n{nic}_1""#)));
    // 数が多い部品には、まとめて載せる印が出る
    assert!(body.contains(&format!(r#"name="register_all_{mem}""#)));
    assert!(body.contains("1 / 18"), "台帳の数が出ていない");
    // **開いただけでは作らない**
    assert_eq!(機器の部品(db, &場).await.len(), 1);
    // 物理ポートは、台帳に載せたNICの分だけ並ぶ
    assert_eq!(body.matches(r#"name="port_"#).count(), 1);
}

/// **シリアル番号が無くても載せられ、あとから同じ画面で入れられること**（6.3）。
async fn シリアルなしで載せてあとから入れる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "sec-register@example.com").await;
    let nic = 載っている型(db, &場).await;
    let mem = 型を作る(db, &場, "Memory", "DIMM-32G").await;
    構成に足す(db, &場, nic, 2).await;
    構成に足す(db, &場, mem, 16).await;

    // 印だけで載せる
    let (status, body) = 部品を送る(db, &場, &[(&format!("register_n{nic}_0"), "1")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let 部品 = 機器の部品(db, &場).await;
    assert_eq!(部品.len(), 2);
    let 新 = 部品.iter().find(|p| p.id != 場.part_instance_id).unwrap();
    assert_eq!(新.serial_number, None);
    assert_eq!(新.status, 場.device.status, "機器と同じ段階で載せる");
    assert_eq!(新.health, "ok");

    // **同じ型が2枚あれば、シリアル番号が空でもポートを別々に割り当てられる**（16.1）
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &詳細(&場), &token).await;
    assert_eq!(body.matches(r#"name="port_"#).count(), 2);
    assert!(
        body.contains("#1 Port1") && body.contains("#2 Port1"),
        "{body}"
    );
    let (status, body) = 節を送る(
        db,
        &場,
        &[
            (
                "port_0",
                &format!("{}:{}", 場.part_instance_id, 場.port_slot_id),
            ),
            ("name_0", "ens1f0"),
            ("port_1", &format!("{}:{}", 新.id, 場.port_slot_id)),
            ("name_1", "ens2f0"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        インターフェース(db, 場.device.id, "ens2f0")
            .await
            .part_instance_id,
        Some(新.id)
    );

    // あとからシリアル番号を入れる。行は作り直さない
    let (status, body) = 部品を送る(db, &場, &[(&format!("serial_p{}", 新.id), " SN-2 ")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let 部品 = 機器の部品(db, &場).await;
    assert_eq!(部品.len(), 2);
    assert_eq!(
        部品
            .iter()
            .find(|p| p.id == 新.id)
            .unwrap()
            .serial_number
            .as_deref(),
        Some("SN-2")
    );

    // **「全部」でまとめて載せる。シリアル番号を入れた行は、印が無くても載せる**
    let (status, body) = 部品を送る(db, &場, &[(&format!("register_all_{mem}"), "1")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(機器の部品(db, &場).await.len(), 18);

    // **構成の数を超えては作らない。**欄の名前を直接送っても増えない
    let (status, _) = 部品を送る(
        db,
        &場,
        &[
            (&format!("register_n{mem}_0"), "1"),
            (&format!("serial_n{nic}_5"), "SN-X"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(機器の部品(db, &場).await.len(), 18);
}

/// **交換はシリアル番号の書き換えで、インターフェースとIPはそのまま残ること**（6.3）。
async fn 交換してもインターフェースが残る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "sec-replace@example.com").await;
    部品を送る(
        db,
        &場,
        &[(&format!("serial_p{}", 場.part_instance_id), "SN-OLD")],
    )
    .await;
    節を送る(
        db,
        &場,
        &[
            (
                "port_0",
                &format!("{}:{}", 場.part_instance_id, 場.port_slot_id),
            ),
            ("name_0", "ens1f0"),
            ("ip_0", "10.0.0.10/24"),
        ],
    )
    .await;
    let 前 = インターフェース(db, 場.device.id, "ens1f0").await;

    let (status, body) = 部品を送る(
        db,
        &場,
        &[
            (&format!("serial_p{}", 場.part_instance_id), "SN-NEW"),
            (&format!("health_p{}", 場.part_instance_id), "ok"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    // 行は同じで、番号だけが変わる
    let 部品 = 機器の部品(db, &場).await;
    assert_eq!(部品.len(), 1);
    assert_eq!(部品[0].id, 場.part_instance_id);
    assert_eq!(部品[0].serial_number.as_deref(), Some("SN-NEW"));
    // インターフェースとIPは閉じていない
    let 後 = インターフェース(db, 場.device.id, "ens1f0").await;
    assert_eq!(後.id, 前.id);
    assert_eq!(現在のip(db, 後.id).await.len(), 1);
    // **前の番号は監査ログに残る**
    let 記録 = audit_log::Entity::find()
        .filter(audit_log::Column::TableName.eq("part_instance"))
        .filter(audit_log::Column::RecordId.eq(場.part_instance_id))
        .filter(audit_log::Column::Action.eq("update"))
        .all(db)
        .await
        .unwrap();
    assert!(記録.iter().any(|r| r
        .before_json
        .as_deref()
        .is_some_and(|j| j.contains("SN-OLD"))
        && r.after_json
            .as_deref()
            .is_some_and(|j| j.contains("SN-NEW"))));

    // 故障の有無も同じ保存で変えられる。語彙に無い値は拒否する
    let (status, _) = 部品を送る(
        db,
        &場,
        &[(&format!("health_p{}", 場.part_instance_id), "failed")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(機器の部品(db, &場).await[0].health, "failed");
    let (status, _) = 部品を送る(
        db,
        &場,
        &[(&format!("health_p{}", 場.part_instance_id), "broken")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(機器の部品(db, &場).await[0].health, "failed");
}

/// **倉庫プロジェクトの予備品と同じ番号に書き換えると、予備品の所在が `Used` になること**
/// （6.3）。`status` は動かさず、行も消さない。
async fn 予備品を使うと使用済みになる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "sec-spare@example.com").await;
    let nic = 載っている型(db, &場).await;
    let 倉庫 = プロジェクト(db, "倉庫").await;
    crate::support::倉庫プロジェクトにする(db, 倉庫.id).await;
    let 予備 = 部品を置く(db, nic, Some("SP-1"), "Project", 倉庫.id).await;

    let (status, body) = 部品を送る(
        db,
        &場,
        &[(&format!("serial_p{}", 場.part_instance_id), "SP-1")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let 所在 = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(予備.id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(所在.len(), 2, "行を消さず、閉じて開く");
    let 今 = 所在.iter().find(|s| s.to_date.is_none()).unwrap();
    assert_eq!(今.location_type, "Used");
    assert_eq!(今.location_id, Some(場.device.id), "使った先の機器");
    assert!(所在
        .iter()
        .any(|s| s.location_type == "Project" && s.to_date.is_some()));
    // `status` は動かさない
    let 後 = part_instance::Entity::find_by_id(予備.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(後.status, 予備.status);
    // 倉庫の在庫からは外れる
    assert!(
        dioryga::part_location::プロジェクトの部品の所在(db, 倉庫.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        機器の部品(db, &場).await[0].serial_number.as_deref(),
        Some("SP-1")
    );

    // **使用済みの部品は、番号の突き合わせに数えない。**同じ保存を繰り返しても通る
    let (status, body) = 部品を送る(
        db,
        &場,
        &[(&format!("serial_p{}", 場.part_instance_id), "SP-1")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
}

/// **他にある番号は拒否し、節を編集のまま返すこと。**倉庫の予備品でない部品を、
/// 黙って使ったことにしない。
async fn 他にある番号は拒否する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "sec-taken@example.com").await;
    let nic = 載っている型(db, &場).await;
    // 倉庫プロジェクトではないプロジェクトにある、同じ型の部品
    let 他 = プロジェクト(db, "他のプロジェクト").await;
    部品を置く(db, nic, Some("SN-OTHER"), "Project", 他.id).await;

    let (status, body) = 部品を送る(
        db,
        &場,
        &[(&format!("serial_p{}", 場.part_instance_id), "SN-OTHER")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に登録されています"), "{body}");
    assert!(
        body.contains(r#"id="edit-parts" checked"#),
        "編集のまま戻っていない"
    );
    assert!(
        body.contains(r#"value="SN-OTHER""#),
        "送った値が残っていない"
    );
    assert_eq!(機器の部品(db, &場).await[0].serial_number, None);

    // 同じ型に同じ番号を2つ入れることもできない
    構成に足す(db, &場, nic, 2).await;
    let (status, body) = 部品を送る(
        db,
        &場,
        &[
            (&format!("serial_p{}", 場.part_instance_id), "SN-1"),
            (&format!("serial_n{nic}_0"), "SN-1"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("同じシリアル番号が2つ"), "{body}");
    assert_eq!(機器の部品(db, &場).await.len(), 1);
}

/// **オンボードのNICは、同じマザーボードのシリアル番号を持つ行が複数あってよいこと**（6.3）。
async fn オンボードは同じ番号を共有できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "sec-onboard@example.com").await;
    let lom = 型を作る(db, &場, "NIC", "LOM").await;
    let bmc = 型を作る(db, &場, "NIC", "BMC").await;
    ポートを足す(db, lom, "Port1").await;
    ポートを足す(db, bmc, "Port1").await;
    構成に足す(db, &場, lom, 1).await;
    構成に足す(db, &場, bmc, 1).await;

    let (status, body) = 部品を送る(
        db,
        &場,
        &[
            (&format!("serial_n{lom}_0"), "MB-1"),
            (&format!("serial_n{bmc}_0"), "MB-1"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let 部品 = 機器の部品(db, &場).await;
    assert_eq!(
        部品
            .iter()
            .filter(|p| p.serial_number.as_deref() == Some("MB-1"))
            .count(),
        2
    );

    // マザーボードを交換したら、両方を書き換える
    let 欄: Vec<(String, &str)> = 部品
        .iter()
        .filter(|p| p.serial_number.is_some())
        .map(|p| (format!("serial_p{}", p.id), "MB-2"))
        .collect();
    let 欄: Vec<(&str, &str)> = 欄.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let (status, body) = 部品を送る(db, &場, &欄).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
}

/// **Viewerは載せられず、他のプロジェクトの機器には書けないこと。**
async fn 閲覧者と部外者は部品を書けない(db: &DatabaseConnection) {
    let 場 = 舞台の役つき(db, "sec-viewer@example.com", "Viewer").await;
    let nic = 載っている型(db, &場).await;
    構成に足す(db, &場, nic, 2).await;
    let (status, _) = 部品を送る(db, &場, &[(&format!("register_n{nic}_0"), "1")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let 他人 = 舞台(db, "sec-outsider@example.com").await;
    let (状態, token) = 認証済み(db, &他人.user).await;
    let (status, _) = 送信(
        状態,
        &format!(
            "/projects/{}/devices/{}/parts",
            他人.project.id, 場.device.id
        ),
        &token,
        &[(&format!("register_n{nic}_0"), "1")],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(機器の部品(db, &場).await.len(), 1);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 部品を送る(
    db: &DatabaseConnection,
    場: &舞台情報,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    let (状態, token) = 認証済み(db, &場.user).await;
    送信(状態, &format!("{}/parts", 詳細(場)), &token, fields).await
}

/// 舞台が機器に載せているNICの型。
async fn 載っている型(db: &DatabaseConnection, 場: &舞台情報) -> i32 {
    part_instance::Entity::find_by_id(場.part_instance_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .part_catalog_id
}

/// 舞台のNICと同じベンダーで、型を足す。
async fn 型を作る(
    db: &DatabaseConnection, 場: &舞台情報, category: &str, name: &str
) -> i32 {
    let 元 = part_catalog::Entity::find_by_id(載っている型(db, 場).await)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    part_catalog::ActiveModel {
        category: Set(category.to_owned()),
        vendor_id: Set(元.vendor_id),
        part_number: Set(format!("{name}-{}", 場.project.id)),
        core_count: Set(None),
        capacity_gb: Set(None),
        spec_json: Set("{}".to_owned()),
        created_by: Set(場.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

async fn ポートを足す(db: &DatabaseConnection, catalog_id: i32, label: &str) {
    part_port_slot::ActiveModel {
        part_catalog_id: Set(catalog_id),
        port_kind: Set("Network".to_owned()),
        port_label: Set(label.to_owned()),
        connector_type: Set("RJ45".to_owned()),
        port_speed: Set(Some("1G".to_owned())),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 構成に足す(
    db: &DatabaseConnection, 場: &舞台情報, catalog_id: i32, quantity: i32
) {
    let d = device::Entity::find_by_id(場.device.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    configuration_part::ActiveModel {
        configuration_id: Set(d.configuration_id.unwrap()),
        part_catalog_id: Set(catalog_id),
        quantity: Set(quantity),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 部品を置く(
    db: &DatabaseConnection,
    catalog_id: i32,
    serial: Option<&str>,
    location_type: &str,
    location_id: i32,
) -> part_instance::Model {
    let p = part_instance::ActiveModel {
        part_catalog_id: Set(catalog_id),
        serial_number: Set(serial.map(str::to_owned)),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    part_instance_location::ActiveModel {
        part_instance_id: Set(p.id),
        location_type: Set(location_type.to_owned()),
        location_id: Set(Some(location_id)),
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    p
}

/// 機器に今載っている部品。
async fn 機器の部品(db: &DatabaseConnection, 場: &舞台情報) -> Vec<part_instance::Model> {
    let ids: Vec<i32> = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq("Device"))
        .filter(part_instance_location::Column::LocationId.eq(場.device.id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|l| l.part_instance_id)
        .collect();
    part_instance::Entity::find()
        .filter(part_instance::Column::Id.is_in(ids))
        .all(db)
        .await
        .unwrap()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 構成の部品が並ぶ);
        全検証!(@one $用意, $属性, シリアルなしで載せてあとから入れる);
        全検証!(@one $用意, $属性, 交換してもインターフェースが残る);
        全検証!(@one $用意, $属性, 予備品を使うと使用済みになる);
        全検証!(@one $用意, $属性, 他にある番号は拒否する);
        全検証!(@one $用意, $属性, オンボードは同じ番号を共有できる);
        全検証!(@one $用意, $属性, 閲覧者と部外者は部品を書けない);
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

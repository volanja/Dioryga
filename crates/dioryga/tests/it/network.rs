//! ネットワーク管理の結合テスト（設計書8.5、8.6、14章）。
//!
//! **8.5が掲げた問いに、画面が実際に答えられるか**を確かめる。
//!
//! > サーバにNICカードが2枚あってポートが計8個ある。各ポートがOSからどう
//! > 見えていて、何のネットワークに繋がっていて、何の仕事をしているのか。
//!
//! 旧設計が答えられなかったのは、**「`OS_INTERFACE` は物理ポートに必ず紐づく」
//! という前提**が原因だった。IPは `bond0` に付き、物理ポートには付かない。
//! ここが崩れていないことを最初に押さえる。

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, chassis_model, configuration, device, device_assignment, interface_role,
    interface_stack, interface_vlan, ip_address, os_interface, part_catalog, part_instance,
    part_instance_location, part_port_slot, project, project_member, subnet, vendor, vlan,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// インターフェース（設計書8.5）
// ---------------------------------------------------------------------------

/// **IPアドレスはボンドに付き、物理ポートには付かないこと**（設計書8.5）。
///
/// 8ポートのNICが4本のbondに束ねられる構成が実務では普通で、旧設計は
/// **IPをどれか1本の物理ポートに恣意的に紐づけるしかなかった。**
async fn ipはボンドに付く(db: &DatabaseConnection) {
    let 場 = 舞台(db, "bond@example.com").await;

    // 物理2本
    for name in ["ens1f0", "ens2f0"] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = インターフェースを送る(
            状態,
            &token,
            &場,
            &[("os_interface_name", name), ("interface_type", "Physical")],
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    }
    // ボンド1本
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = インターフェースを送る(
        状態,
        &token,
        &場,
        &[
            ("os_interface_name", "bond0"),
            ("interface_type", "Bond"),
            ("aggregation_mode", "LACP"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let bond = インターフェース(db, 場.device.id, "bond0").await;
    for lower in ["ens1f0", "ens2f0"] {
        let l = インターフェース(db, 場.device.id, lower).await;
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = 送信(
            状態,
            &経路(&場, "members"),
            &token,
            &[
                ("os_interface_id", &bond.id.to_string()),
                ("lower_interface_id", &l.id.to_string()),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    }

    // **IPはボンドに付ける**
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = ipを送る(状態, &token, &場, bond.id, "10.0.0.10", "24", None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let ips = 現在のip(db, bond.id).await;
    assert_eq!(ips.len(), 1);
    assert_eq!(ips[0].ip_address, "10.0.0.10");

    // 画面に構成が出る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &経路(&場, ""), &token).await;
    assert!(body.contains("bond0"));
    assert!(body.contains("ens1f0, ens2f0"), "束ねた下位が出ていない");
    assert!(body.contains("10.0.0.10/24"));
}

/// **物理ポートを持てるのは Physical だけであること**（設計書8.5）。
///
/// VMには `PART_INSTANCE` が無く、bondも物理ポートに1対1で対応しない。
async fn 物理ポートはphysicalだけ(db: &DatabaseConnection) {
    let 場 = 舞台(db, "port-kind@example.com").await;
    let port = 空きポートの値(&場);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = インターフェースを送る(
        状態,
        &token,
        &場,
        &[
            ("os_interface_name", "bond0"),
            ("interface_type", "Bond"),
            ("port", &port),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Physical のインターフェースだけ"));

    // Physical なら通る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = インターフェースを送る(
        状態,
        &token,
        &場,
        &[
            ("os_interface_name", "ens1f0"),
            ("interface_type", "Physical"),
            ("port", &port),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let i = インターフェース(db, 場.device.id, "ens1f0").await;
    assert_eq!(i.part_instance_id, Some(場.part_instance_id));
    assert_eq!(i.port_slot_id, Some(場.port_slot_id));
}

/// **`aggregation_mode` を持てるのは Bond だけであること**（設計書8.5）。
async fn 束ね方はbondだけ(db: &DatabaseConnection) {
    let 場 = 舞台(db, "agg@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = インターフェースを送る(
        状態,
        &token,
        &場,
        &[
            ("os_interface_name", "ens1f0"),
            ("interface_type", "Physical"),
            ("aggregation_mode", "LACP"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Bond のインターフェースだけ"));
}

/// **VMには物理ポートが無くてもIPを持てること**（設計書8.5(2)）。
///
/// 13章でVMを `DEVICE` として扱えるようにしたが、**VMには `PART_INSTANCE` が
/// 無いため、旧設計ではVMのIPを記録する場所がなかった。**
async fn 仮想インターフェースにもipを付けられる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "virtual@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = インターフェースを送る(
        状態,
        &token,
        &場,
        &[("os_interface_name", "eth0"), ("interface_type", "Virtual")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let i = インターフェース(db, 場.device.id, "eth0").await;
    assert!(i.part_instance_id.is_none());

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = ipを送る(状態, &token, &場, i.id, "192.168.1.20", "24", None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(現在のip(db, i.id).await.len(), 1);
}

/// **束ねる関係が循環しないこと。**
///
/// 中間テーブルで両方向を扱う以上、`bond0` の下に `bond0.100` を入れることも
/// 形の上では書けてしまう。上下を辿る表示が終わらなくなる。
async fn 循環する束ねは拒否される(db: &DatabaseConnection) {
    let 場 = 舞台(db, "cycle@example.com").await;
    let a = 作る(db, &場, "bond0", "Bond").await;
    let b = 作る(db, &場, "bond0.100", "Vlan").await;

    // bond0.100 の下に bond0
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 束ねる(状態, &token, &場, b.id, a.id).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // その逆は循環になる
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 束ねる(状態, &token, &場, a.id, b.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("循環"));

    // 自分自身も拒否する
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 束ねる(状態, &token, &場, a.id, a.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("自分自身"));
}

// ---------------------------------------------------------------------------
// VLAN（設計書8.6）
// ---------------------------------------------------------------------------

/// **ポートVLANとタグVLANを同じ形で扱えること**（設計書8.6）。
///
/// トランクポートは**ネイティブVLANの Untagged と、タグVLANの Tagged が並ぶ。**
/// 単一のFKでは表せないのがこの表を作った理由である。
async fn トランクは複数のvlanを載せられる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "trunk@example.com").await;
    let i = 作る(db, &場, "Eth1/1/1", "Physical").await;

    let native = vlanを作る(db, &場, 1, "native").await;
    let a = vlanを作る(db, &場, 100, "業務").await;
    let b = vlanを作る(db, &場, 200, "管理").await;

    for (v, mode) in [(native, "Untagged"), (a, "Tagged"), (b, "Tagged")] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = vlanを載せる(状態, &token, &場, i.id, v, mode).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    }

    let 載っている = 現在のvlan(db, i.id).await;
    assert_eq!(載っている.len(), 3);
    assert_eq!(
        載っている
            .iter()
            .filter(|l| l.tagging_mode == "Tagged")
            .count(),
        2
    );

    // **同じVLANは2行載せない**
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = vlanを載せる(状態, &token, &場, i.id, a, "Untagged").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に載っています"));
}

// ---------------------------------------------------------------------------
// サブネットとゾーン（設計書14章、8.5）
// ---------------------------------------------------------------------------

/// **ゾーンは `SUBNET` を優先し、無ければVLAN側を見ること**（設計書14.2）。
async fn ゾーンはサブネットを優先する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "zone@example.com").await;
    let v = vlanを作る一式(db, &場, 100, "業務", Some("LAN")).await;

    // VLANだけにゾーンがある
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = サブネットを送る(
        状態,
        &token,
        &場,
        &[("cidr", "10.0.0.0/24"), ("vlan_id", &v.to_string())],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &サブネット経路(&場), &token).await;
    assert!(body.contains("LAN"));
    assert!(body.contains("VLAN由来"), "由来が示されていない");

    // サブネット側にゾーンがあれば、そちらが勝つ
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = サブネットを送る(
        状態,
        &token,
        &場,
        &[
            ("cidr", "10.0.1.0/24"),
            ("vlan_id", &v.to_string()),
            ("zone", "DMZ"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let s = subnet::Entity::find()
        .filter(subnet::Column::Cidr.eq("10.0.1.0/24"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.zone.as_deref(), Some("DMZ"));
}

/// **プロジェクトが違えば同じアドレス帯を持てること**（設計書14.2）。
///
/// サブネットを `project_id` 単位に分けた理由がここにある。
async fn 別プロジェクトなら同じcidrを持てる(db: &DatabaseConnection) {
    let a = 舞台(db, "cidr-a@example.com").await;
    let b = 舞台(db, "cidr-b@example.com").await;

    for 場 in [&a, &b] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) =
            サブネットを送る(状態, &token, 場, &[("cidr", "192.168.1.0/24")]).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    }
    assert_eq!(
        subnet::Entity::find()
            .filter(subnet::Column::Cidr.eq("192.168.1.0/24"))
            .all(db)
            .await
            .unwrap()
            .len(),
        2
    );

    // 同じプロジェクト内では重複させない
    let (状態, token) = 認証済み(db, &a.user).await;
    let (status, body) = サブネットを送る(状態, &token, &a, &[("cidr", "192.168.1.0/24")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に登録されています"));
}

/// **同一サブネット内でIPを重複させないこと**（設計書14.2）。
async fn 同じサブネットのipは重複できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "ipdup@example.com").await;
    let i = 作る(db, &場, "eth0", "Virtual").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    サブネットを送る(状態, &token, &場, &[("cidr", "10.0.0.0/24")]).await;
    let s = subnet::Entity::find()
        .filter(subnet::Column::Cidr.eq("10.0.0.0/24"))
        .one(db)
        .await
        .unwrap()
        .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = ipを送る(状態, &token, &場, i.id, "10.0.0.10", "24", Some(s.id)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let j = 作る(db, &場, "eth1", "Virtual").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = ipを送る(状態, &token, &場, j.id, "10.0.0.10", "24", Some(s.id)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既にあります"));
}

// ---------------------------------------------------------------------------
// 閉じる（設計書4章）
// ---------------------------------------------------------------------------

/// **消さずに閉じること。上に載っているものも一緒に閉じること。**
///
/// 閉じたインターフェースにIPがぶら下がったままだと、現在の状態を計算した
/// ときに存在しないインターフェースのIPが出る。
async fn インターフェースを閉じると上も閉じる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "close@example.com").await;
    let bond = 作る(db, &場, "bond0", "Bond").await;
    let lower = 作る(db, &場, "ens1f0", "Physical").await;
    let v = vlanを作る(db, &場, 100, "業務").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    束ねる(状態, &token, &場, bond.id, lower.id).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    vlanを載せる(状態, &token, &場, bond.id, v, "Tagged").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    ipを送る(状態, &token, &場, bond.id, "10.0.0.10", "24", None).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    役割を送る(状態, &token, &場, bond.id, "Backup").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 閉じる(状態, &token, &場, "interface", bond.id).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // **行は消えていない**（4章）
    assert_eq!(
        ip_address::Entity::find().all(db).await.unwrap().len(),
        1,
        "IPの行が消えている"
    );
    // すべて閉じている
    assert!(現在のip(db, bond.id).await.is_empty());
    assert!(現在のvlan(db, bond.id).await.is_empty());
    assert!(現在の役割(db, bond.id).await.is_empty());
    assert!(現在の束ね(db).await.is_empty(), "束ねが残っている");

    // 下位は生きている
    assert!(現在のインターフェース(db, 場.device.id)
        .await
        .iter()
        .any(|i| i.os_interface_name == "ens1f0"));
}

// ---------------------------------------------------------------------------
// 権限・可視性
// ---------------------------------------------------------------------------

/// **Viewerは編集できないこと。**閲覧はできる（3章）。
async fn 閲覧者はネットワークを編集できない(db: &DatabaseConnection) {
    let 場 = 舞台の役つき(db, "net-viewer@example.com", "Viewer").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = インターフェースを送る(
        状態,
        &token,
        &場,
        &[("os_interface_name", "eth0"), ("interface_type", "Virtual")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 取得(状態, &経路(&場, ""), &token).await;
    assert_eq!(status, StatusCode::OK);
}

/// **他プロジェクトの機器のインターフェースは開けないこと**（A-6）。
async fn 部外者は機器のインターフェースを開けない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "net-owner@example.com").await;
    let 他人 = 舞台(db, "net-outsider@example.com").await;

    let (状態, token) = 認証済み(db, &他人.user).await;
    let (status, _) = 取得(
        状態,
        &format!(
            "/projects/{}/devices/{}/interfaces",
            他人.project.id, 場.device.id
        ),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
    device: device::Model,
    part_instance_id: i32,
    port_slot_id: i32,
}

async fn 舞台(db: &DatabaseConnection, email: &str) -> 舞台情報 {
    舞台の役つき(db, email, "Operator").await
}

async fn 舞台の役つき(db: &DatabaseConnection, email: &str, role: &str) -> 舞台情報 {
    let user = 利用者(db, email).await;
    let p = プロジェクト(db, &format!("{email}のプロジェクト")).await;
    メンバー(db, user.id, p.id, role).await;

    let v = vendor::ActiveModel {
        name: Set(format!("ベンダー-{email}")),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let m = chassis_model::ActiveModel {
        vendor_id: Set(v.id),
        model_name: Set(format!("型-{email}")),
        device_category: Set("Server".to_owned()),
        height_u: Set(1),
        mount_form: Set("RackU".to_owned()),
        rack_width: Set(Some("Full".to_owned())),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let c = configuration::ActiveModel {
        chassis_model_id: Set(m.id),
        name: Set("標準構成".to_owned()),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(Some(c.id)),
        hostname: Set(format!("srv-{}", p.id)),
        device_type: Set("Physical".to_owned()),
        power_watt: Set(350),
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
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // NICを1枚載せ、ネットワークポートを1本持たせる（8.5の経路）
    let nic = part_catalog::ActiveModel {
        category: Set("NIC".to_owned()),
        vendor_id: Set(v.id),
        part_number: Set(format!("NIC-{}", p.id)),
        core_count: Set(None),
        capacity_gb: Set(None),
        spec_json: Set("{}".to_owned()),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let slot = part_port_slot::ActiveModel {
        part_catalog_id: Set(nic.id),
        port_kind: Set("Network".to_owned()),
        port_label: Set("Port1".to_owned()),
        connector_type: Set("SFP28".to_owned()),
        port_speed: Set(Some("25G".to_owned())),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let instance = part_instance::ActiveModel {
        part_catalog_id: Set(nic.id),
        serial_number: Set(None),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    part_instance_location::ActiveModel {
        part_instance_id: Set(instance.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(d.id)),
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    舞台情報 {
        user,
        project: p,
        device: d,
        part_instance_id: instance.id,
        port_slot_id: slot.id,
    }
}

fn 経路(場: &舞台情報, sub: &str) -> String {
    let base = format!(
        "/projects/{}/devices/{}/interfaces",
        場.project.id, 場.device.id
    );
    if sub.is_empty() {
        base
    } else {
        format!("{base}/{sub}")
    }
}

fn サブネット経路(場: &舞台情報) -> String {
    format!("/projects/{}/network/subnets", 場.project.id)
}

fn 空きポートの値(場: &舞台情報) -> String {
    format!("{}:{}", 場.part_instance_id, 場.port_slot_id)
}

async fn インターフェースを送る(
    state: AppState,
    token: &str,
    場: &舞台情報,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(state, &経路(場, ""), token, fields).await
}

async fn 作る(
    db: &DatabaseConnection,
    場: &舞台情報,
    name: &str,
    kind: &str,
) -> os_interface::Model {
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = インターフェースを送る(
        状態,
        &token,
        場,
        &[("os_interface_name", name), ("interface_type", kind)],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    インターフェース(db, 場.device.id, name).await
}

async fn 束ねる(
    state: AppState,
    token: &str,
    場: &舞台情報,
    upper: i32,
    lower: i32,
) -> (StatusCode, String) {
    送信(
        state,
        &経路(場, "members"),
        token,
        &[
            ("os_interface_id", &upper.to_string()),
            ("lower_interface_id", &lower.to_string()),
        ],
    )
    .await
}

async fn vlanを載せる(
    state: AppState,
    token: &str,
    場: &舞台情報,
    interface_id: i32,
    vlan_id: i32,
    mode: &str,
) -> (StatusCode, String) {
    送信(
        state,
        &経路(場, "vlans"),
        token,
        &[
            ("os_interface_id", &interface_id.to_string()),
            ("vlan_id", &vlan_id.to_string()),
            ("tagging_mode", mode),
        ],
    )
    .await
}

async fn 役割を送る(
    state: AppState,
    token: &str,
    場: &舞台情報,
    interface_id: i32,
    role: &str,
) -> (StatusCode, String) {
    送信(
        state,
        &経路(場, "roles"),
        token,
        &[
            ("os_interface_id", &interface_id.to_string()),
            ("role", role),
        ],
    )
    .await
}

async fn ipを送る(
    state: AppState,
    token: &str,
    場: &舞台情報,
    interface_id: i32,
    address: &str,
    prefix: &str,
    subnet_id: Option<i32>,
) -> (StatusCode, String) {
    送信(
        state,
        &経路(場, "ips"),
        token,
        &[
            ("os_interface_id", &interface_id.to_string()),
            ("ip_address", address),
            ("prefix_length", prefix),
            (
                "subnet_id",
                &subnet_id.map(|i| i.to_string()).unwrap_or_default(),
            ),
        ],
    )
    .await
}

async fn 閉じる(
    state: AppState,
    token: &str,
    場: &舞台情報,
    target: &str,
    id: i32,
) -> (StatusCode, String) {
    送信(
        state,
        &経路(場, "close"),
        token,
        &[("target", target), ("id", &id.to_string())],
    )
    .await
}

async fn サブネットを送る(
    state: AppState,
    token: &str,
    場: &舞台情報,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    送信(state, &サブネット経路(場), token, fields).await
}

async fn vlanを作る(db: &DatabaseConnection, 場: &舞台情報, tag: i32, name: &str) -> i32 {
    vlanを作る一式(db, 場, tag, name, None).await
}

async fn vlanを作る一式(
    db: &DatabaseConnection,
    場: &舞台情報,
    tag: i32,
    name: &str,
    zone: Option<&str>,
) -> i32 {
    vlan::ActiveModel {
        vlan_tag: Set(tag),
        name: Set(name.to_owned()),
        zone: Set(zone.map(str::to_owned)),
        description: Set(String::new()),
        retired_at: Set(None),
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

async fn インターフェース(
    db: &DatabaseConnection,
    device_id: i32,
    name: &str,
) -> os_interface::Model {
    os_interface::Entity::find()
        .filter(os_interface::Column::DeviceId.eq(device_id))
        .filter(os_interface::Column::OsInterfaceName.eq(name))
        .filter(os_interface::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{name} が見つかりません"))
}

async fn 現在のインターフェース(
    db: &DatabaseConnection,
    device_id: i32,
) -> Vec<os_interface::Model> {
    os_interface::Entity::find()
        .filter(os_interface::Column::DeviceId.eq(device_id))
        .filter(os_interface::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap()
}

async fn 現在のip(db: &DatabaseConnection, interface_id: i32) -> Vec<ip_address::Model> {
    ip_address::Entity::find()
        .filter(ip_address::Column::OsInterfaceId.eq(interface_id))
        .filter(ip_address::Column::ToDate.is_null())
        .order_by_asc(ip_address::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 現在のvlan(db: &DatabaseConnection, interface_id: i32) -> Vec<interface_vlan::Model> {
    interface_vlan::Entity::find()
        .filter(interface_vlan::Column::OsInterfaceId.eq(interface_id))
        .filter(interface_vlan::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap()
}

async fn 現在の役割(db: &DatabaseConnection, interface_id: i32) -> Vec<interface_role::Model> {
    interface_role::Entity::find()
        .filter(interface_role::Column::OsInterfaceId.eq(interface_id))
        .filter(interface_role::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap()
}

async fn 現在の束ね(db: &DatabaseConnection) -> Vec<interface_stack::Model> {
    interface_stack::Entity::find()
        .filter(interface_stack::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap()
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

async fn プロジェクト(db: &DatabaseConnection, name: &str) -> project::Model {
    project::ActiveModel {
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
    .unwrap()
}

async fn メンバー(db: &DatabaseConnection, user_id: i32, project_id: i32, role: &str) {
    project_member::ActiveModel {
        project_id: Set(project_id),
        user_id: Set(user_id),
        role: Set(role.to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
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
        全検証!(@one $用意, $属性, ipはボンドに付く);
        全検証!(@one $用意, $属性, 物理ポートはphysicalだけ);
        全検証!(@one $用意, $属性, 束ね方はbondだけ);
        全検証!(@one $用意, $属性, 仮想インターフェースにもipを付けられる);
        全検証!(@one $用意, $属性, 循環する束ねは拒否される);
        全検証!(@one $用意, $属性, トランクは複数のvlanを載せられる);
        全検証!(@one $用意, $属性, ゾーンはサブネットを優先する);
        全検証!(@one $用意, $属性, 別プロジェクトなら同じcidrを持てる);
        全検証!(@one $用意, $属性, 同じサブネットのipは重複できない);
        全検証!(@one $用意, $属性, インターフェースを閉じると上も閉じる);
        全検証!(@one $用意, $属性, 閲覧者はネットワークを編集できない);
        全検証!(@one $用意, $属性, 部外者は機器のインターフェースを開けない);
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

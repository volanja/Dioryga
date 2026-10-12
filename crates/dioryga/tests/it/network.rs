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
// インターフェース（設計書8.5）。機器の詳細の「ポートとIP」の節で保存する（16.1）
// ---------------------------------------------------------------------------

/// **IPアドレスはボンドに付き、物理ポートには付かないこと**（設計書8.5）。
///
/// 8ポートのNICが4本のbondに束ねられる構成が実務では普通で、旧設計は
/// **IPをどれか1本の物理ポートに恣意的に紐づけるしかなかった。**
///
/// **物理ポート・Bond・束ね・IPを、1回の保存で設定できること**（16.1）。
async fn ipはボンドに付く(db: &DatabaseConnection) {
    let 場 = 舞台(db, "bond@example.com").await;
    let port = 空きポートの値(&場);

    let (status, body) = 節を送る(
        db,
        &場,
        &[
            // 台帳に載せたNICのポート。束ね先に、同じ保存で作るBondを書く
            ("port_0", &port),
            ("name_0", "ens1f0"),
            ("stack_0", "bond0"),
            // ポートの分からない物理インターフェース
            ("name_1", "ens2f0"),
            ("kind_1", "Physical"),
            ("name_2", "bond0"),
            ("kind_2", "Bond:LACP"),
            ("stack_2", "ens2f0"),
            ("ip_2", "10.0.0.10/24"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let bond = インターフェース(db, 場.device.id, "bond0").await;
    assert_eq!(bond.aggregation_mode.as_deref(), Some("LACP"));
    let 物理 = インターフェース(db, 場.device.id, "ens1f0").await;
    assert_eq!(物理.part_instance_id, Some(場.part_instance_id));
    assert_eq!(物理.port_slot_id, Some(場.port_slot_id));

    // **IPはボンドに付く**
    let ips = 現在のip(db, bond.id).await;
    assert_eq!(ips.len(), 1);
    assert_eq!(ips[0].ip_address, "10.0.0.10");
    assert!(現在のip(db, 物理.id).await.is_empty());

    let 束ね = 現在の束ね(db).await;
    assert_eq!(束ね.len(), 2);
    assert!(束ね.iter().all(|s| s.upper_interface_id == bond.id));

    // 機器の詳細に構成が出る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &詳細(&場), &token).await;
    assert!(body.contains("bond0"));
    assert!(body.contains("→ bond0"), "束ね先が出ていない");
    assert!(body.contains("10.0.0.10/24"));
}

/// **物理ポートを持つ行は、必ず Physical になること**（設計書8.5）。
///
/// VMには `PART_INSTANCE` が無く、bondも物理ポートに1対1で対応しない。
async fn 物理ポートはphysicalだけ(db: &DatabaseConnection) {
    let 場 = 舞台(db, "port-kind@example.com").await;
    let port = 空きポートの値(&場);

    // 種別を送っても、物理ポートの行は Physical で作る
    let (status, body) = 節を送る(
        db,
        &場,
        &[
            ("port_0", &port),
            ("name_0", "ens1f0"),
            ("kind_0", "Bond:LACP"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let i = インターフェース(db, 場.device.id, "ens1f0").await;
    assert_eq!(i.interface_type, "Physical");
    assert_eq!(i.aggregation_mode, None);
    assert_eq!(i.part_instance_id, Some(場.part_instance_id));
    assert_eq!(i.port_slot_id, Some(場.port_slot_id));

    // 同じポートに2本目は作れない
    let (status, body) = 節を送る(db, &場, &[("port_0", &port), ("name_0", "ens1f9")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既にインターフェースがあります"), "{body}");

    // この機器のものでないポートは受け付けない
    let (status, body) =
        節を送る(db, &場, &[("port_0", "999999:999999"), ("name_0", "ens9")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("物理ポートの指定が不正"), "{body}");
}

/// **束ね方を持てるのは Bond だけであること**（設計書8.5）。閉じた語彙は既定へ寄せない。
async fn 束ね方はbondだけ(db: &DatabaseConnection) {
    let 場 = 舞台(db, "agg@example.com").await;

    for kind in ["Physical:LACP", "Bond:Unknown"] {
        let (status, body) = 節を送る(db, &場, &[("name_0", "x0"), ("kind_0", kind)]).await;
        assert_eq!(status, StatusCode::OK, "{kind}");
        assert!(body.contains("束ね方の値が不正"), "{kind}");
    }
    let (status, body) = 節を送る(db, &場, &[("name_0", "x0"), ("kind_0", "Tunnel")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("種別の値が不正"));
    assert!(現在のインターフェース(db, 場.device.id).await.is_empty());

    // **検証エラーでは、節を編集のまま、送った値を残して返す**（16.1）
    assert!(
        body.contains(r#"id="edit-ports" checked"#),
        "編集のまま戻っていない"
    );
    assert!(body.contains(r#"value="x0""#), "送った値が残っていない");
}

/// **VMには物理ポートが無くてもIPを持てること**（設計書8.5(2)）。
///
/// 13章でVMを `DEVICE` として扱えるようにしたが、**VMには `PART_INSTANCE` が
/// 無いため、旧設計ではVMのIPを記録する場所がなかった。**
async fn 仮想インターフェースにもipを付けられる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "virtual@example.com").await;

    let (status, body) = 節を送る(
        db,
        &場,
        &[
            ("name_0", "eth0"),
            ("kind_0", "Virtual"),
            ("ip_0", "192.168.1.20/24"),
            ("role_0", "Service, Backup"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let i = インターフェース(db, 場.device.id, "eth0").await;
    assert!(i.part_instance_id.is_none());
    assert_eq!(現在のip(db, i.id).await.len(), 1);
    assert_eq!(現在の役割(db, i.id).await.len(), 2);

    // プレフィックスの無いIPは受け付けない
    let (status, body) = 行を送る(db, &場, i.id, &[("ip", "192.168.1.21")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("プレフィックスまで"), "{body}");
}

/// **束ねる関係が循環しないこと。**
///
/// 中間テーブルで両方向を扱う以上、`bond0` の下に `bond0.100` を入れることも
/// 形の上では書けてしまう。上下を辿る表示が終わらなくなる。
async fn 循環する束ねは拒否される(db: &DatabaseConnection) {
    let 場 = 舞台(db, "cycle@example.com").await;

    // bond0.100 の下に bond0
    let (status, body) = 節を送る(
        db,
        &場,
        &[
            ("name_0", "bond0"),
            ("kind_0", "Bond"),
            ("name_1", "bond0.100"),
            ("kind_1", "Vlan"),
            ("stack_1", "bond0"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let a = インターフェース(db, 場.device.id, "bond0").await;
    assert_eq!(現在の束ね(db).await.len(), 1);

    // その逆は循環になる
    let (status, body) = 行を送る(db, &場, a.id, &[("stack", "bond0.100")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("循環"));

    // 自分自身も拒否する
    let (status, body) = 行を送る(db, &場, a.id, &[("stack", "bond0")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("自分自身"));

    // 無い名前も拒否する
    let (status, body) = 行を送る(db, &場, a.id, &[("stack", "bond9")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("名前のインターフェースがありません"),
        "{body}"
    );
    assert_eq!(現在の束ね(db).await.len(), 1);
}

// ---------------------------------------------------------------------------
// VLAN（設計書8.6）
// ---------------------------------------------------------------------------

/// **ポートVLANとタグVLANを同じ形で扱えること**（設計書8.6）。
///
/// トランクポートは**ネイティブVLANの Untagged と、タグVLANの Tagged が並ぶ。**
/// 単一のFKでは表せないのがこの表を作った理由である。画面では番号で入れ、
/// タグ付きは末尾に `T` を付ける（16.1）。
async fn トランクは複数のvlanを載せられる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "trunk@example.com").await;
    vlanを作る(db, &場, 1, "native").await;
    let a = vlanを作る(db, &場, 100, "業務").await;
    vlanを作る(db, &場, 200, "管理").await;

    let (status, body) = 節を送る(
        db,
        &場,
        &[
            ("name_0", "Eth1/1/1"),
            ("kind_0", "Physical"),
            ("vlan_0", "1, 100T, 200T"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let i = インターフェース(db, 場.device.id, "Eth1/1/1").await;

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
    let (status, body) = 行を送る(db, &場, i.id, &[("vlan", "100, 100T")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に載っています"));

    // 無い番号は受け付けない
    let (status, body) = 行を送る(db, &場, i.id, &[("vlan", "1, 999")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("その番号のVLANがありません"), "{body}");
    assert_eq!(現在のvlan(db, i.id).await.len(), 3);

    // **減らした分だけを閉じ、タグの付け方が変わった行は閉じて開く。行は消さない**
    let (status, body) = 行を送る(db, &場, i.id, &[("vlan", "1, 100")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let 載っている = 現在のvlan(db, i.id).await;
    assert_eq!(載っている.len(), 2);
    assert!(載っている
        .iter()
        .any(|l| l.vlan_id == a && l.tagging_mode == "Untagged"));
    assert_eq!(
        interface_vlan::Entity::find().all(db).await.unwrap().len(),
        4,
        "閉じた行が消えている"
    );

    // **同じ番号のVLANが複数あれば、黙って選ばない**（VLANタグは一意でない。8.3）
    vlanを作る(db, &場, 100, "別拠点").await;
    let (status, body) = 行を送る(db, &場, i.id, &[("vlan", "1, 100")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("同じ番号のVLANが複数"), "{body}");
    let (status, body) = 行を送る(db, &場, i.id, &[("vlan", "1, 100:業務")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
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
///
/// **サブネットはアドレスから決まること**（16.1）。画面は選ばせない。
async fn 同じサブネットのipは重複できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "ipdup@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    サブネットを送る(状態, &token, &場, &[("cidr", "10.0.0.0/24")]).await;
    let s = subnet::Entity::find()
        .filter(subnet::Column::Cidr.eq("10.0.0.0/24"))
        .one(db)
        .await
        .unwrap()
        .unwrap();

    let (status, body) = 節を送る(
        db,
        &場,
        &[
            ("name_0", "eth0"),
            ("kind_0", "Virtual"),
            ("ip_0", "10.0.0.10/24, 172.16.0.1/24"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let i = インターフェース(db, 場.device.id, "eth0").await;
    let ips = 現在のip(db, i.id).await;
    assert_eq!(ips[0].subnet_id, Some(s.id), "サブネットが決まっていない");
    // 含むサブネットが無いアドレスは、サブネット無しで記録する
    assert_eq!(ips[1].subnet_id, None);

    let (status, body) = 節を送る(
        db,
        &場,
        &[
            ("name_0", "eth1"),
            ("kind_0", "Virtual"),
            ("ip_0", "10.0.0.10/24"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既にあります"));

    // **同じ保存で外すなら、付け替えられる**
    let (status, body) = 節を送る(
        db,
        &場,
        &[
            ("id_0", &i.id.to_string()),
            ("name_0", "eth0"),
            ("kind_0", "Virtual"),
            ("ip_0", "172.16.0.1/24"),
            ("name_1", "eth1"),
            ("kind_1", "Virtual"),
            ("ip_1", "10.0.0.10/24"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let j = インターフェース(db, 場.device.id, "eth1").await;
    assert_eq!(現在のip(db, j.id).await.len(), 1);
    assert_eq!(現在のip(db, i.id).await.len(), 1);
}

// ---------------------------------------------------------------------------
// 外す（設計書4章、8.5）
// ---------------------------------------------------------------------------

/// **消さずに閉じること。上に載っているものも一緒に閉じること。**
///
/// 閉じたインターフェースにIPがぶら下がったままだと、現在の状態を計算した
/// ときに存在しないインターフェースのIPが出る。
async fn インターフェースを閉じると上も閉じる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "close@example.com").await;
    vlanを作る(db, &場, 100, "業務").await;

    let (status, body) = 節を送る(
        db,
        &場,
        &[
            ("name_0", "ens1f0"),
            ("kind_0", "Physical"),
            ("name_1", "bond0"),
            ("kind_1", "Bond"),
            ("stack_1", "ens1f0"),
            ("vlan_1", "100T"),
            ("ip_1", "10.0.0.10/24"),
            ("role_1", "Backup"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let bond = インターフェース(db, 場.device.id, "bond0").await;
    assert_eq!(現在の束ね(db).await.len(), 1);

    let (status, body) = 行を送る(db, &場, bond.id, &[("remove", "1")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    // **行は消えていない**（4章）
    assert_eq!(
        ip_address::Entity::find().all(db).await.unwrap().len(),
        1,
        "IPの行が消えている"
    );
    assert_eq!(os_interface::Entity::find().all(db).await.unwrap().len(), 2);
    // すべて閉じている
    assert!(現在のip(db, bond.id).await.is_empty());
    assert!(現在のvlan(db, bond.id).await.is_empty());
    assert!(現在の役割(db, bond.id).await.is_empty());
    assert!(現在の束ね(db).await.is_empty(), "束ねが残っている");

    // 下位は生きている
    let 残り = 現在のインターフェース(db, 場.device.id).await;
    assert_eq!(残り.len(), 1);
    assert_eq!(残り[0].os_interface_name, "ens1f0");
}

/// **名前や種別を変えたら、書き換えずに閉じて開き直すこと**（不変条件1、設計書23.5）。
///
/// 載っていたものは、新しい行に付け直す。
async fn 名前を変えると閉じて開き直す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "rename@example.com").await;
    節を送る(
        db,
        &場,
        &[
            ("name_0", "eth0"),
            ("kind_0", "Virtual"),
            ("ip_0", "10.0.0.10/24"),
        ],
    )
    .await;
    let 前 = インターフェース(db, 場.device.id, "eth0").await;

    let (status, body) = 行を送る(db, &場, 前.id, &[("name", "eth9")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let 後 = インターフェース(db, 場.device.id, "eth9").await;
    assert_ne!(前.id, 後.id, "行を書き換えている");
    let 全部 = os_interface::Entity::find().all(db).await.unwrap();
    assert_eq!(全部.len(), 2);
    assert!(全部
        .iter()
        .any(|i| i.id == 前.id && i.os_interface_name == "eth0" && i.to_date.is_some()));
    assert!(現在のip(db, 前.id).await.is_empty());
    assert_eq!(現在のip(db, 後.id).await[0].ip_address, "10.0.0.10");
}

/// **変わらない保存は何も書かず、送らなかった行には触れないこと。**
///
/// 画面を開いて何も変えずに保存しただけで、履歴に行が増えてはならない。
async fn 変わらない行には触れない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "noop@example.com").await;
    節を送る(
        db,
        &場,
        &[
            ("name_0", "eth0"),
            ("kind_0", "Virtual"),
            ("ip_0", "10.0.0.10/24"),
            ("role_0", "Service"),
            ("name_1", "eth1"),
            ("kind_1", "Virtual"),
            ("ip_1", "10.0.0.11/24"),
        ],
    )
    .await;
    let a = インターフェース(db, 場.device.id, "eth0").await;
    let b = インターフェース(db, 場.device.id, "eth1").await;
    let 監査 = entity::audit_log::Entity::find()
        .all(db)
        .await
        .unwrap()
        .len();

    // 同じ内容をもう一度送る
    let (status, body) = 行を送る(db, &場, a.id, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        entity::audit_log::Entity::find()
            .all(db)
            .await
            .unwrap()
            .len(),
        監査,
        "変わらない保存で書き込んでいる"
    );

    // eth0 の行だけを送って役割を外す。eth1 は残る
    let (status, body) = 行を送る(db, &場, a.id, &[("role", "")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(現在の役割(db, a.id).await.is_empty());
    assert_eq!(現在のip(db, a.id).await.len(), 1);
    assert_eq!(現在のip(db, b.id).await.len(), 1);
    assert_eq!(現在のインターフェース(db, 場.device.id).await.len(), 2);
}

// ---------------------------------------------------------------------------
// 権限・可視性
// ---------------------------------------------------------------------------

/// **Viewerは編集できないこと。**閲覧はできる（3章）。
async fn 閲覧者はネットワークを編集できない(db: &DatabaseConnection) {
    let 場 = 舞台の役つき(db, "net-viewer@example.com", "Viewer").await;

    let (status, _) = 節を送る(db, &場, &[("name_0", "eth0"), ("kind_0", "Virtual")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, &詳細(&場), &token).await;
    assert_eq!(status, StatusCode::OK);
    // 編集の導線を出さない
    assert!(!body.contains(r#"id="edit-ports""#));
    assert!(!body.contains(r#"id="edit-parts""#));
}

/// **他プロジェクトの機器のインターフェースは開けず、書けないこと**（A-6）。
async fn 部外者は機器のインターフェースを開けない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "net-owner@example.com").await;
    let 他人 = 舞台(db, "net-outsider@example.com").await;
    let 経路 = format!("/projects/{}/devices/{}", 他人.project.id, 場.device.id);

    let (状態, token) = 認証済み(db, &他人.user).await;
    let (status, _) = 取得(状態, &経路, &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (状態, token) = 認証済み(db, &他人.user).await;
    let (status, _) = 送信(
        状態,
        &format!("{経路}/interfaces"),
        &token,
        &[("name_0", "eth0"), ("kind_0", "Virtual")],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(現在のインターフェース(db, 場.device.id).await.is_empty());

    // 自分の機器でも、別の機器のインターフェースのIDは受け付けない
    節を送る(db, &場, &[("name_0", "eth0"), ("kind_0", "Virtual")]).await;
    let 他の = インターフェース(db, 場.device.id, "eth0").await;
    let (status, _) = 節を送る(
        db,
        &他人,
        &[
            ("id_0", &他の.id.to_string()),
            ("name_0", "eth0"),
            ("remove_0", "1"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(現在のインターフェース(db, 場.device.id).await.len(), 1);
}

/// 以前のインターフェースの画面の場所は、機器の詳細の節へ送る。
async fn 以前の画面の場所は節へ送る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "old-url@example.com").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let res = router(状態)
        .oneshot(
            Request::builder()
                .uri(経路(&場))
                .header(header::COOKIE, cookie_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get(header::LOCATION).unwrap(),
        &format!("{}#ports", 詳細(&場))
    );
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// NICを1枚載せた機器と、その持ち主。`device_sections.rs` も使う。
pub(super) struct 舞台情報 {
    pub user: app_user::Model,
    pub project: project::Model,
    pub device: device::Model,
    pub part_instance_id: i32,
    pub port_slot_id: i32,
}

pub(super) async fn 舞台(db: &DatabaseConnection, email: &str) -> 舞台情報 {
    舞台の役つき(db, email, "Operator").await
}

pub(super) async fn 舞台の役つき(
    db: &DatabaseConnection,
    email: &str,
    role: &str,
) -> 舞台情報 {
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

/// ポートとIPの節の送り先。
fn 経路(場: &舞台情報) -> String {
    format!("{}/interfaces", 詳細(場))
}

pub(super) fn 詳細(場: &舞台情報) -> String {
    format!("/projects/{}/devices/{}", 場.project.id, 場.device.id)
}

fn サブネット経路(場: &舞台情報) -> String {
    format!("/projects/{}/network/subnets", 場.project.id)
}

fn 空きポートの値(場: &舞台情報) -> String {
    format!("{}:{}", 場.part_instance_id, 場.port_slot_id)
}

/// ポートとIPの節を送る。`name_0` のように、行の番号つきの名前で渡す。
pub(super) async fn 節を送る(
    db: &DatabaseConnection,
    場: &舞台情報,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    let (状態, token) = 認証済み(db, &場.user).await;
    送信(状態, &経路(場), &token, fields).await
}

/// 既存の1行だけを送る。**今の状態をそのまま入力欄の値にし、`変更` で上書きする。**
/// 画面が今の値を入れて出すのと同じ形である。
async fn 行を送る(
    db: &DatabaseConnection,
    場: &舞台情報,
    interface_id: i32,
    変更: &[(&str, &str)],
) -> (StatusCode, String) {
    let i = os_interface::Entity::find_by_id(interface_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let 名前 = |id: i32| async move {
        os_interface::Entity::find_by_id(id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .os_interface_name
    };
    let ポート行 = i.port_slot_id.is_some();
    let mut 束ね = Vec::new();
    for s in 現在の束ね(db).await {
        // 物理ポートの行は束ね先（上位）を、それ以外は下位を持つ
        if ポート行 && s.lower_interface_id == i.id {
            束ね.push(名前(s.upper_interface_id).await);
        } else if !ポート行 && s.upper_interface_id == i.id {
            束ね.push(名前(s.lower_interface_id).await);
        }
    }
    let mut vlans = Vec::new();
    for link in 現在のvlan(db, i.id).await {
        let v = vlan::Entity::find_by_id(link.vlan_id)
            .one(db)
            .await
            .unwrap()
            .unwrap();
        vlans.push(match link.tagging_mode.as_str() {
            "Tagged" => format!("{}T", v.vlan_tag),
            _ => v.vlan_tag.to_string(),
        });
    }
    let mut 値: Vec<(String, String)> = vec![
        ("id".to_owned(), i.id.to_string()),
        ("name".to_owned(), i.os_interface_name.clone()),
        (
            "kind".to_owned(),
            match &i.aggregation_mode {
                Some(m) => format!("{}:{m}", i.interface_type),
                None => i.interface_type.clone(),
            },
        ),
        ("stack".to_owned(), 束ね.join(", ")),
        ("vlan".to_owned(), vlans.join(", ")),
        (
            "ip".to_owned(),
            現在のip(db, i.id)
                .await
                .iter()
                .map(|ip| format!("{}/{}", ip.ip_address, ip.prefix_length))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        (
            "role".to_owned(),
            現在の役割(db, i.id)
                .await
                .iter()
                .map(|r| r.role.clone())
                .collect::<Vec<_>>()
                .join(", "),
        ),
    ];
    for (key, value) in 変更 {
        match 値.iter_mut().find(|(k, _)| k == key) {
            Some(組) => 組.1 = (*value).to_owned(),
            None => 値.push(((*key).to_owned(), (*value).to_owned())),
        }
    }
    let 値: Vec<(String, String)> = 値.into_iter().map(|(k, v)| (format!("{k}_0"), v)).collect();
    let fields: Vec<(&str, &str)> = 値.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    節を送る(db, 場, &fields).await
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

pub(super) async fn インターフェース(
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

pub(super) async fn 現在のip(
    db: &DatabaseConnection,
    interface_id: i32,
) -> Vec<ip_address::Model> {
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

pub(super) async fn プロジェクト(db: &DatabaseConnection, name: &str) -> project::Model {
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

pub(super) async fn 認証済み(
    db: &DatabaseConnection,
    user: &app_user::Model,
) -> (AppState, String) {
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

pub(super) async fn 取得(state: AppState, uri: &str, token: &str) -> (StatusCode, String) {
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

pub(super) async fn 送信(
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
        全検証!(@one $用意, $属性, 名前を変えると閉じて開き直す);
        全検証!(@one $用意, $属性, 変わらない行には触れない);
        全検証!(@one $用意, $属性, 以前の画面の場所は節へ送る);
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

//! ポート接続の結合テスト（設計書16.1、8.4、8.7）。
//!
//! ケーブルの端を、機器に載っている部品のポートに挿す。**ケーブルの実物は
//! 最初の接続と同時に作り、プロジェクトへの所属は持たない。**

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, cable_catalog, cable_connection, cable_end_slot, cable_instance, device,
    device_assignment, os_interface, part_catalog, part_instance, part_instance_location,
    part_port_slot, project, project_member, vendor, work_order,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;
use tower::ServiceExt;

/// **新しいケーブルを挿すと実物ができ、反対の端は相手の機器から挿せること。**
async fn 両端を挿すと接続先が出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-both@example.com", "Operator").await;
    let a = 機器(db, &場, "conn-a", "Network", "LC").await;
    let b = 機器(db, &場, "conn-b", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;

    // A端を conn-a に挿す。実物はここでできる
    let (status, body) = 挿す(
        db,
        &場,
        &a,
        &format!("new:{}", 型.ends[0].id),
        &[("serial_number", "CBL-001")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let 実物 = cable_instance::Entity::find().all(db).await.unwrap();
    assert_eq!(実物.len(), 1);
    assert_eq!(実物[0].serial_number.as_deref(), Some("CBL-001"));
    let 接続 = 現行の接続(db).await;
    assert_eq!(接続.len(), 1);
    // この画面が書く接続は、変更管理チケットを持たない
    assert_eq!(接続[0].work_order_id, None);

    // 反対の端はまだ挿していない
    let (_, body) = 画面(db, &場, &a).await;
    assert!(body.contains("CBL-001"), "{body}");
    assert!(body.contains("B: 未接続"), "{body}");

    // conn-b の画面に、空いた端が候補として出る
    let 空いた端 = format!("open:{}:{}", 実物[0].id, 型.ends[1].id);
    let (_, body) = 画面(db, &場, &b).await;
    assert!(
        body.contains(&空いた端),
        "空いた端が候補に出ていません: {body}"
    );

    let (status, body) = 挿す(db, &場, &b, &空いた端, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    // 実物は増えない
    assert_eq!(
        cable_instance::Entity::find().all(db).await.unwrap().len(),
        1
    );
    assert_eq!(現行の接続(db).await.len(), 2);

    // どちらの画面にも、相手の機器が出る
    let (_, body) = 画面(db, &場, &a).await;
    assert!(body.contains("B: conn-b"), "{body}");
    let (_, body) = 画面(db, &場, &b).await;
    assert!(body.contains("A: conn-a"), "{body}");
    // 両端が埋まったので、候補から消える
    assert!(!body.contains("open:"), "{body}");
}

/// **外すと現行行を閉じ、行は残ること。**すべての端を外したケーブルは、
/// 候補に出ない（画面から辿れなくなる）。
async fn 外すと閉じて残る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-off@example.com", "Operator").await;
    let a = 機器(db, &場, "off-a", "Network", "LC").await;
    let b = 機器(db, &場, "off-b", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    let c = 現行の接続(db).await.remove(0);

    // 別の機器の画面からは外せない
    let (status, _) = 外す(db, &場, &b, c.id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(現行の接続(db).await.len(), 1);

    let (status, body) = 外す(db, &場, &a, c.id).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(現行の接続(db).await.is_empty());
    let 履歴 = cable_connection::Entity::find().all(db).await.unwrap();
    assert_eq!(履歴.len(), 1, "行が消えています");
    assert!(履歴[0].to_date.is_some());

    // 実物は残るが、どの画面からも辿れない
    assert_eq!(
        cable_instance::Entity::find().all(db).await.unwrap().len(),
        1
    );
    let (_, body) = 画面(db, &場, &b).await;
    assert!(!body.contains("open:"), "{body}");

    // ポートは空いたので、もう一度挿せる（新しい実物になる）
    let (status, _) = 挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        cable_instance::Entity::find().all(db).await.unwrap().len(),
        2
    );
}

/// **ケーブルの種別とポートの種別が違えば拒否すること。**
async fn 種別が違えば挿せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-kind@example.com", "Operator").await;
    let a = 機器(db, &場, "kind-a", "Network", "LC").await;
    let 電源 = ケーブルの型(db, &場, "Power", &[("A", "IEC C13"), ("B", "IEC C14")]).await;

    let (status, body) = 挿す(db, &場, &a, &format!("new:{}", 電源.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("種別が違います"), "{body}");
    assert!(現行の接続(db).await.is_empty());
    assert!(cable_instance::Entity::find()
        .all(db)
        .await
        .unwrap()
        .is_empty());
}

/// **コネクタ形状が違っても挿せて、警告が出ること。**対になる組（C13とC14）は
/// 値が違うので、形状では拒否しない。
async fn 形状が違えば警告を出す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-shape@example.com", "Operator").await;
    let psu = 機器(db, &場, "shape-a", "Power", "IEC C14").await;
    let nic = 機器(db, &場, "shape-b", "Network", "LC").await;
    let 電源 = ケーブルの型(db, &場, "Power", &[("A", "IEC C13"), ("B", "NEMA 5-15P")]).await;
    let 光 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;

    let (status, body) = 挿す(db, &場, &psu, &format!("new:{}", 電源.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let (_, body) = 画面(db, &場, &psu).await;
    assert!(body.contains("コネクタ形状が違います"), "{body}");

    // 同じ値なら警告は出ない
    挿す(db, &場, &nic, &format!("new:{}", 光.ends[0].id), &[]).await;
    let (_, body) = 画面(db, &場, &nic).await;
    assert!(!body.contains("コネクタ形状が違います"), "{body}");
}

/// **使用中のポートと端には挿せないこと。**
async fn 使用中のポートと端には挿せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-used@example.com", "Operator").await;
    let a = 機器(db, &場, "used-a", "Network", "LC").await;
    let b = 機器(db, &場, "used-b", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();

    // 同じポートにもう1本
    let (status, body) = 挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("すでにケーブルが挿さっています"), "{body}");

    // すでに挿さっている端を、別の機器に
    let (status, body) = 挿す(
        db,
        &場,
        &b,
        &format!("open:{}:{}", 実物.id, 型.ends[0].id),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("すでに挿さっています"), "{body}");

    // 別の型の端は、このケーブルの端ではない
    let 他 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    let (status, _) = 挿す(
        db,
        &場,
        &b,
        &format!("open:{}:{}", 実物.id, 他.ends[1].id),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(現行の接続(db).await.len(), 1);
}

/// **他のプロジェクトのケーブルの端は挿せないこと。**IDを直接送っても通さない。
async fn 他のプロジェクトのケーブルは挿せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-mine@example.com", "Operator").await;
    let 他 = 舞台(db, "conn-other@example.com", "Operator").await;
    let a = 機器(db, &場, "mine-a", "Network", "LC").await;
    let x = 機器(db, &他, "other-x", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &他, &x, &format!("new:{}", 型.ends[0].id), &[]).await;
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let 空いた端 = format!("open:{}:{}", 実物.id, 型.ends[1].id);

    // 候補に出ない
    let (_, body) = 画面(db, &場, &a).await;
    assert!(!body.contains(&空いた端), "{body}");
    assert!(!body.contains("other-x"), "{body}");

    let (status, _) = 挿す(db, &場, &a, &空いた端, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(現行の接続(db).await.len(), 1);
}

/// **閲覧だけのロールは、画面は開けるが挿せないこと。**
async fn 閲覧者は挿せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-view@example.com", "Viewer").await;
    let a = 機器(db, &場, "view-a", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;

    let (status, body) = 画面(db, &場, &a).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("name=\"cable\""),
        "接続のフォームが出ています"
    );

    let (status, _) = 挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(現行の接続(db).await.is_empty());
}

/// **ブレイクアウトの端を、1本ずつ別々に挿せること。**
async fn ブレイクアウトの端を別々に挿せる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-bo@example.com", "Operator").await;
    let sw = 機器(db, &場, "bo-sw", "Network", "MPO12").await;
    let s1 = 機器(db, &場, "bo-s1", "Network", "LC").await;
    let s2 = 機器(db, &場, "bo-s2", "Network", "LC").await;
    let 型 = ケーブルの型(
        db,
        &場,
        "Network",
        &[("Trunk", "MPO12"), ("Branch1", "LC"), ("Branch2", "LC")],
    )
    .await;

    挿す(db, &場, &sw, &format!("new:{}", 型.ends[0].id), &[]).await;
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let (s, _) = 挿す(
        db,
        &場,
        &s1,
        &format!("open:{}:{}", 実物.id, 型.ends[1].id),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);

    // 1本挿しても、残りの端は候補に残る
    let 残り = format!("open:{}:{}", 実物.id, 型.ends[2].id);
    let (_, body) = 画面(db, &場, &s2).await;
    assert!(body.contains(&残り), "{body}");
    let (s, _) = 挿す(db, &場, &s2, &残り, &[]).await;
    assert_eq!(s, StatusCode::SEE_OTHER);

    let (_, body) = 画面(db, &場, &sw).await;
    assert!(body.contains("Branch1: bo-s1"), "{body}");
    assert!(body.contains("Branch2: bo-s2"), "{body}");
}

/// **プロジェクトをまたぐ接続は、両方で編集できる人だけが挿せること。**
///
/// 相手の機器は、そのプロジェクトのメンバーにだけ見せる。メンバーでない人には
/// 「別のプロジェクトの機器」とだけ出す。
async fn またぐ接続は両方の権限が要る(db: &DatabaseConnection) {
    let 共用 = 舞台(db, "conn-shared@example.com", "Operator").await;
    let 場 = 舞台(db, "conn-cross@example.com", "Operator").await;
    let sw = 機器(db, &共用, "cross-sw", "Network", "LC").await;
    let srv = 機器(db, &場, "cross-srv", "Network", "LC").await;
    let 型 = ケーブルの型(db, &共用, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &共用, &sw, &format!("new:{}", 型.ends[0].id), &[]).await;
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let 空いた端 = format!("open:{}:{}", 実物.id, 型.ends[1].id);

    // 相手のプロジェクトでは閲覧だけ → 候補に出ず、送っても挿せない
    メンバー(db, 場.user.id, 共用.project.id, "Viewer").await;
    let (_, body) = 画面(db, &場, &srv).await;
    assert!(!body.contains(&空いた端), "{body}");
    let (status, _) = 挿す(db, &場, &srv, &空いた端, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(現行の接続(db).await.len(), 1);

    // 両方で編集できる → 候補に出て、挿せる
    メンバー(db, 場.user.id, 共用.project.id, "Operator").await;
    let (_, body) = 画面(db, &場, &srv).await;
    assert!(body.contains(&空いた端), "{body}");
    let (status, body) = 挿す(db, &場, &srv, &空いた端, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(現行の接続(db).await.len(), 2);

    // 相手のプロジェクトのメンバーには、機器名とプロジェクト名が出る
    let (_, body) = 画面(db, &場, &srv).await;
    assert!(body.contains("A: cross-sw"), "{body}");
    assert!(body.contains(&共用.project.name), "{body}");

    // メンバーでない人には、「別のプロジェクトの機器」とだけ出る
    let 閲覧者 = 利用者(db, "conn-cross-viewer@example.com").await;
    メンバー(db, 閲覧者.id, 場.project.id, "Viewer").await;
    let (状態, token) = 認証済み(db, &閲覧者).await;
    let (_, body) = 取得(状態, &経路(&場, &srv), &token).await;
    assert!(body.contains("別のプロジェクトの機器"), "{body}");
    assert!(!body.contains("cross-sw"), "機器名が見えています: {body}");
    assert!(!body.contains(&共用.project.name), "{body}");

    // 共用の側だけの担当者も、自分の側の端は外せる
    let c = 現行の接続(db)
        .await
        .into_iter()
        .find(|c| c.part_instance_id == sw.part_instance_id)
        .unwrap();
    let (status, _) = 外す(db, &共用, &sw, c.id).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(現行の接続(db).await.len(), 1);
}

/// **同じ型番の部品が複数載っていれば、シリアル番号で見分けられること。**
/// シリアル番号が無ければ、載せた順の番号を添える。
async fn 同じ型番はシリアル番号で見分ける(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-serial@example.com", "Operator").await;
    let 一枚 = 機器(db, &場, "serial-one", "Network", "LC").await;
    let 二枚 = 機器(db, &場, "serial-two", "Network", "LC").await;
    同じ型の部品を足す(db, &二枚, Some("SN-B")).await;

    // 1枚だけなら型番のまま
    let (_, body) = 画面(db, &場, &一枚).await;
    assert!(!body.contains("PN-serial-one #"), "{body}");
    assert!(!body.contains("PN-serial-one ["), "{body}");

    // 2枚なら、シリアル番号（無ければ番号）を添える
    let (_, body) = 画面(db, &場, &二枚).await;
    assert!(body.contains("PN-serial-two #1"), "{body}");
    assert!(body.contains("PN-serial-two [SN-B]"), "{body}");

    // 接続先にも同じ見分けが出る
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &場, &二枚, &format!("new:{}", 型.ends[0].id), &[]).await;
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    挿す(
        db,
        &場,
        &一枚,
        &format!("open:{}:{}", 実物.id, 型.ends[1].id),
        &[],
    )
    .await;
    let (_, body) = 画面(db, &場, &一枚).await;
    assert!(
        body.contains("A: serial-two PN-serial-two #1 Port1"),
        "{body}"
    );
}

/// **インターフェース一覧に、物理ポートの接続先が出ること。**
async fn インターフェース一覧に接続先が出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-if@example.com", "Operator").await;
    let a = 機器(db, &場, "if-a", "Network", "LC").await;
    let b = 機器(db, &場, "if-b", "Network", "LC").await;
    os_interface::ActiveModel {
        device_id: Set(a.device.id),
        interface_type: Set("Physical".to_owned()),
        part_instance_id: Set(Some(a.part_instance_id)),
        port_slot_id: Set(Some(a.port_slot_id)),
        os_interface_name: Set("eth0".to_owned()),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let 一覧 = format!(
        "/projects/{}/devices/{}/interfaces",
        場.project.id, a.device.id
    );

    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    // 反対の端を挿す前は、ケーブルの名前と「未接続」
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &一覧, &token).await;
    assert!(body.contains("（未接続）"), "{body}");

    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    挿す(
        db,
        &場,
        &b,
        &format!("open:{}:{}", 実物.id, 型.ends[1].id),
        &[],
    )
    .await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &一覧, &token).await;
    assert!(body.contains("if-b PN-if-b Port1"), "{body}");
}

/// **機器の詳細に、給電経路の本数が出ること**（設計書12.6）。電源ポートを持つ
/// 部品が載っていない機器には出さない。
async fn 給電経路の本数が出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-feed@example.com", "Operator").await;
    let 電源つき = 機器(db, &場, "feed-a", "Power", "IEC C14").await;
    同じ型の部品を足す(db, &電源つき, None).await;
    let 電源なし = 機器(db, &場, "feed-b", "Network", "LC").await;
    let 詳細 = |d: &機器情報| format!("/projects/{}/devices/{}", 場.project.id, d.device.id);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &詳細(&電源つき), &token).await;
    assert!(body.contains("0本（電源ポート 2口のうち）"), "{body}");

    let 型 = ケーブルの型(db, &場, "Power", &[("A", "IEC C13"), ("B", "IEC C14")]).await;
    挿す(db, &場, &電源つき, &format!("new:{}", 型.ends[0].id), &[]).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &詳細(&電源つき), &token).await;
    assert!(body.contains("1本（電源ポート 2口のうち）"), "{body}");

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &詳細(&電源なし), &token).await;
    assert!(!body.contains("給電経路"), "{body}");
}

/// **増設チケットを選んで挿すと予約になり、実行で実際の接続になること**（11.6）。
///
/// 予約したポートと端は埋まる。チケットが実行されるまで「予約中」と出る。
async fn 予約は実行で実際の接続になる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-rsv@example.com", "Operator").await;
    let 新 = 機器(db, &場, "rsv-new", "Network", "LC").await;
    let sw = 機器(db, &場, "rsv-sw", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    let w = チケット(db, &場, "Addition", "planned").await;
    let 票 = w.id.to_string();

    // 候補に出る
    let (_, body) = 画面(db, &場, &新).await;
    assert!(body.contains("name=\"work_order_id\""), "{body}");
    assert!(body.contains(&w.title), "{body}");

    // 両端を、同じチケットの予約として挿す
    let (status, body) = 挿す(
        db,
        &場,
        &新,
        &format!("new:{}", 型.ends[0].id),
        &[("work_order_id", &票)],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    挿す(
        db,
        &場,
        &sw,
        &format!("open:{}:{}", 実物.id, 型.ends[1].id),
        &[("work_order_id", &票)],
    )
    .await;
    let 接続 = 現行の接続(db).await;
    assert_eq!(接続.len(), 2);
    assert!(接続.iter().all(|c| c.work_order_id == Some(w.id)));

    // 予約中と出る。相手側にも出る
    let (_, body) = 画面(db, &場, &新).await;
    assert!(body.contains("badge plan"), "{body}");
    assert!(
        body.contains("B: rsv-sw PN-rsv-sw Port1（予約中）"),
        "{body}"
    );

    // 予約したポートには、別の接続を登録できない
    let (status, body) = 挿す(db, &場, &sw, &format!("new:{}", 型.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("すでにケーブルが挿さっています"), "{body}");

    // チケットの詳細に出る
    let 詳細 = format!("/projects/{}/work-orders/{}", 場.project.id, w.id);
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &詳細, &token).await;
    assert!(body.contains("rsv-new PN-rsv-new Port1"), "{body}");
    assert!(body.contains("rsv-sw PN-rsv-sw Port1"), "{body}");

    // 実行されると、予約ではなくなる。行は書き換えない
    チケットの状態(db, w.id, "in_progress").await;
    let (_, body) = 画面(db, &場, &新).await;
    assert!(!body.contains("badge plan"), "{body}");
    assert!(!body.contains("（予約中）"), "{body}");
    assert_eq!(現行の接続(db).await.len(), 2);
}

/// **中断すると、予約した接続が解放されること**（11.6）。行は消さず閉じる。
async fn 中断すると予約が解放される(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-abort@example.com", "Operator").await;
    let a = 機器(db, &場, "abort-a", "Network", "LC").await;
    let b = 機器(db, &場, "abort-b", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    let w = チケット(db, &場, "Addition", "planned").await;
    let 票 = w.id.to_string();

    挿す(
        db,
        &場,
        &a,
        &format!("new:{}", 型.ends[0].id),
        &[("work_order_id", &票)],
    )
    .await;
    // チケットを選ばずに挿した接続は、予約ではない
    挿す(db, &場, &b, &format!("new:{}", 型.ends[0].id), &[]).await;
    assert_eq!(現行の接続(db).await.len(), 2);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 送信(
        状態,
        &format!(
            "/projects/{}/work-orders/{}/transition",
            場.project.id, w.id
        ),
        &token,
        &[("to", "abort"), ("cancelled_reason", "計画を取りやめた")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    // 予約だけが閉じる
    let 残り = 現行の接続(db).await;
    assert_eq!(残り.len(), 1);
    assert_eq!(残り[0].part_instance_id, b.part_instance_id);
    assert_eq!(
        cable_connection::Entity::find()
            .all(db)
            .await
            .unwrap()
            .len(),
        2
    );

    // ポートは空き、チケットの詳細には解放済みとして残る
    let (_, body) = 画面(db, &場, &a).await;
    assert!(body.contains("つながっていません"), "{body}");
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/work-orders/{}", 場.project.id, w.id),
        &token,
    )
    .await;
    assert!(body.contains("abort-a PN-abort-a Port1"), "{body}");
    assert!(body.contains("解放済み"), "{body}");
}

/// **予約に使えるのは、このプロジェクトの計画中の増設チケットだけ**（11.6）。
async fn 予約は計画中の増設にしか作れない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-rsv-ng@example.com", "Operator").await;
    let 他 = 舞台(db, "conn-rsv-other@example.com", "Operator").await;
    let a = 機器(db, &場, "rsv-ng-a", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;

    for w in [
        チケット(db, &場, "Addition", "approved").await,
        チケット(db, &場, "Relocation", "planned").await,
        チケット(db, &他, "Addition", "planned").await,
    ] {
        let (_, body) = 画面(db, &場, &a).await;
        assert!(
            !body.contains(&format!("<option value=\"{}\">{}", w.id, w.title)),
            "候補に出ています: {} {}",
            w.work_type,
            w.status
        );
        let (status, body) = 挿す(
            db,
            &場,
            &a,
            &format!("new:{}", 型.ends[0].id),
            &[("work_order_id", &w.id.to_string())],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{} {}", w.work_type, w.status);
        assert!(body.contains("計画中の増設チケットだけ"), "{body}");
    }
    assert!(現行の接続(db).await.is_empty());
    assert!(cable_instance::Entity::find()
        .all(db)
        .await
        .unwrap()
        .is_empty());
}

/// **予約は給電経路の本数に数えないこと。**まだ挿していない。
async fn 予約は給電経路に数えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-rsv-feed@example.com", "Operator").await;
    let a = 機器(db, &場, "rsv-feed", "Power", "IEC C14").await;
    let 型 = ケーブルの型(db, &場, "Power", &[("A", "IEC C13"), ("B", "IEC C14")]).await;
    let w = チケット(db, &場, "Addition", "planned").await;
    挿す(
        db,
        &場,
        &a,
        &format!("new:{}", 型.ends[0].id),
        &[("work_order_id", &w.id.to_string())],
    )
    .await;
    let 詳細 = format!("/projects/{}/devices/{}", 場.project.id, a.device.id);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &詳細, &token).await;
    assert!(body.contains("0本（電源ポート 1口のうち）"), "{body}");

    チケットの状態(db, w.id, "in_progress").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(状態, &詳細, &token).await;
    assert!(body.contains("1本（電源ポート 1口のうち）"), "{body}");
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
    vendor_id: i32,
}

/// ポートを1つ持つ部品を載せた機器。
struct 機器情報 {
    device: device::Model,
    part_instance_id: i32,
    port_slot_id: i32,
}

struct 型情報 {
    ends: Vec<cable_end_slot::Model>,
}

async fn 舞台(db: &DatabaseConnection, email: &str, role: &str) -> 舞台情報 {
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
    舞台情報 {
        user,
        project: p,
        vendor_id: v.id,
    }
}

/// 機器を作り、ポートを1つ持つ部品を載せる。
async fn 機器(
    db: &DatabaseConnection,
    場: &舞台情報,
    hostname: &str,
    port_kind: &str,
    connector: &str,
) -> 機器情報 {
    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(None),
        hostname: Set(hostname.to_owned()),
        device_type: Set("Physical".to_owned()),
        device_category: Set(Some("Server".to_owned())),
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
        location_id: Set(Some(場.project.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let catalog = part_catalog::ActiveModel {
        category: Set("NIC".to_owned()),
        vendor_id: Set(場.vendor_id),
        part_number: Set(format!("PN-{hostname}")),
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
    .unwrap();
    let slot = part_port_slot::ActiveModel {
        part_catalog_id: Set(catalog.id),
        port_kind: Set(port_kind.to_owned()),
        port_label: Set("Port1".to_owned()),
        connector_type: Set(connector.to_owned()),
        port_speed: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let instance = part_instance::ActiveModel {
        part_catalog_id: Set(catalog.id),
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

    機器情報 {
        device: d,
        part_instance_id: instance.id,
        port_slot_id: slot.id,
    }
}

/// 機器に、同じ型の部品をもう1つ載せる。
async fn 同じ型の部品を足す(
    db: &DatabaseConnection, d: &機器情報, serial: Option<&str>
) {
    let 元 = part_instance::Entity::find_by_id(d.part_instance_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let instance = part_instance::ActiveModel {
        part_catalog_id: Set(元.part_catalog_id),
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
        part_instance_id: Set(instance.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(d.device.id)),
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn チケット(
    db: &DatabaseConnection,
    場: &舞台情報,
    work_type: &str,
    status: &str,
) -> work_order::Model {
    work_order::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        project_id: Set(場.project.id),
        target_project_id: Set(None),
        work_type: Set(work_type.to_owned()),
        title: Set(format!("{work_type}-{status}のチケット")),
        description: Set(String::new()),
        primary_assignee_id: Set(None),
        status: Set(status.to_owned()),
        planned_at: Set(Some(Utc::now())),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn チケットの状態(db: &DatabaseConnection, id: i32, status: &str) {
    let w = work_order::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut active: work_order::ActiveModel = w.into();
    active.status = Set(status.to_owned());
    active.update(db).await.unwrap();
}

async fn ケーブルの型(
    db: &DatabaseConnection,
    場: &舞台情報,
    kind: &str,
    ends: &[(&str, &str)],
) -> 型情報 {
    let c = cable_catalog::ActiveModel {
        cable_kind: Set(Some(kind.to_owned())),
        cable_type: Set(format!("{kind}ケーブル")),
        length_mm: Set(Some(2000)),
        color: Set(String::new()),
        vendor_id: Set(None),
        part_number: Set(None),
        rated_voltage: Set(None),
        rated_current_ma: Set(None),
        retired_at: Set(None),
        created_by: Set(場.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let mut out = Vec::new();
    for (label, connector) in ends {
        out.push(
            cable_end_slot::ActiveModel {
                cable_catalog_id: Set(c.id),
                end_label: Set((*label).to_owned()),
                connector_type: Set((*connector).to_owned()),
                port_speed: Set(None),
                created_at: Set(Utc::now()),
                updated_at: Set(Utc::now()),
                ..Default::default()
            }
            .insert(db)
            .await
            .unwrap(),
        );
    }
    型情報 { ends: out }
}

fn 経路(場: &舞台情報, d: &機器情報) -> String {
    format!(
        "/projects/{}/devices/{}/connections",
        場.project.id, d.device.id
    )
}

async fn 画面(
    db: &DatabaseConnection, 場: &舞台情報, d: &機器情報
) -> (StatusCode, String) {
    let (状態, token) = 認証済み(db, &場.user).await;
    取得(状態, &経路(場, d), &token).await
}

async fn 挿す(
    db: &DatabaseConnection,
    場: &舞台情報,
    d: &機器情報,
    cable: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, String) {
    let port = format!("{}:{}", d.part_instance_id, d.port_slot_id);
    let mut fields = vec![("port", port.as_str()), ("cable", cable)];
    fields.extend_from_slice(extra);
    let (状態, token) = 認証済み(db, &場.user).await;
    送信(状態, &経路(場, d), &token, &fields).await
}

async fn 外す(
    db: &DatabaseConnection,
    場: &舞台情報,
    d: &機器情報,
    connection_id: i32,
) -> (StatusCode, String) {
    let id = connection_id.to_string();
    let (状態, token) = 認証済み(db, &場.user).await;
    送信(
        状態,
        &format!("{}/disconnect", 経路(場, d)),
        &token,
        &[("connection_id", id.as_str())],
    )
    .await
}

async fn 現行の接続(db: &DatabaseConnection) -> Vec<cable_connection::Model> {
    cable_connection::Entity::find()
        .filter(cable_connection::Column::ToDate.is_null())
        .order_by_asc(cable_connection::Column::Id)
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
        全検証!(@one $用意, $属性, 両端を挿すと接続先が出る);
        全検証!(@one $用意, $属性, 外すと閉じて残る);
        全検証!(@one $用意, $属性, 種別が違えば挿せない);
        全検証!(@one $用意, $属性, 形状が違えば警告を出す);
        全検証!(@one $用意, $属性, 使用中のポートと端には挿せない);
        全検証!(@one $用意, $属性, 他のプロジェクトのケーブルは挿せない);
        全検証!(@one $用意, $属性, 閲覧者は挿せない);
        全検証!(@one $用意, $属性, ブレイクアウトの端を別々に挿せる);
        全検証!(@one $用意, $属性, またぐ接続は両方の権限が要る);
        全検証!(@one $用意, $属性, 同じ型番はシリアル番号で見分ける);
        全検証!(@one $用意, $属性, インターフェース一覧に接続先が出る);
        全検証!(@one $用意, $属性, 給電経路の本数が出る);
        全検証!(@one $用意, $属性, 予約は実行で実際の接続になる);
        全検証!(@one $用意, $属性, 中断すると予約が解放される);
        全検証!(@one $用意, $属性, 予約は計画中の増設にしか作れない);
        全検証!(@one $用意, $属性, 予約は給電経路に数えない);
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

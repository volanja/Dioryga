//! プロジェクト領域の機器画面の結合テスト（設計書16.1のB領域、3章、A-6）。

mod support;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::{Duration, Utc};
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, chassis_model, configuration, device, device_assignment, fixed_asset,
    maintenance_contract, maintenance_contract_item, project, project_member, purchase, vendor,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// クロスプロジェクト可視性（A-6）
// ---------------------------------------------------------------------------

/// **過去に所属した機器も閲覧できること**（A-6、設計書3章）。
///
/// 現在の割当だけで絞ると、移設された機器の履歴が誰からも見えなくなる。
/// 既定の一覧では現役に絞るが、範囲を広げれば出る。
async fn 過去に所属した機器も閲覧できる(db: &DatabaseConnection) {
    let user = 利用者(db, "a6@example.com").await;
    let 旧 = プロジェクト(db, "移管元").await;
    let 新 = プロジェクト(db, "移管先").await;
    メンバー(db, user.id, 旧.id, "Viewer").await;

    let d = 機器(db, "moved-01").await;
    // 旧プロジェクトに居た → 新プロジェクトへ移した
    let 昔 = 割当(db, d.id, 旧.id, Utc::now() - Duration::days(30)).await;
    閉じる(db, 昔).await;
    割当(db, d.id, 新.id, Utc::now()).await;

    // 既定（現在このプロジェクトにあるもの）には出ない
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, &format!("/projects/{}/devices", 旧.id), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("moved-01"), "既定で移管済みが出ています");

    // 範囲を広げると出る。**これがA-6の要点**
    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/devices?scope=all", 旧.id),
        &token,
    )
    .await;
    assert!(
        body.contains("moved-01"),
        "過去に所属した機器が見えません（A-6違反）"
    );

    // 詳細も開ける。所在の履歴が両方出る
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", 旧.id, d.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("移管元"));
    assert!(body.contains("移管先"), "移設後の所在が見えません");
}

/// **所属したことのないプロジェクトからは見えないこと。**
///
/// URLを直接叩かれても通さない。A-6は「一度でも所属したか」で判定する。
async fn 無関係なプロジェクトからは見えない(db: &DatabaseConnection) {
    let user = 利用者(db, "unrelated@example.com").await;
    let 自分の = プロジェクト(db, "自分のプロジェクト").await;
    let 他人の = プロジェクト(db, "他人のプロジェクト").await;
    メンバー(db, user.id, 自分の.id, "Administrator").await;

    let d = 機器(db, "secret-01").await;
    割当(db, d.id, 他人の.id, Utc::now()).await;

    // 一覧に出ない
    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/devices?scope=all", 自分の.id),
        &token,
    )
    .await;
    assert!(!body.contains("secret-01"));

    // IDを直接指定しても見えない
    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", 自分の.id, d.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 所属していないプロジェクトの画面自体に入れないこと。
async fn 非メンバーは入れない(db: &DatabaseConnection) {
    let user = 利用者(db, "nonmember@example.com").await;
    let p = プロジェクト(db, "よそのプロジェクト").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// ロールによる編集の可否
// ---------------------------------------------------------------------------

/// **ViewerとApproverは編集できないこと。**
///
/// Approverは承認する立場であり、自分で変更を入れられると承認の意味が薄れる。
async fn 閲覧のみのロールは登録できない(db: &DatabaseConnection) {
    for (email, role) in [
        ("viewer@example.com", "Viewer"),
        ("approver@example.com", "Approver"),
    ] {
        let user = 利用者(db, email).await;
        let p = プロジェクト(db, &format!("{role}検証")).await;
        メンバー(db, user.id, p.id, role).await;

        // 登録画面に入れない
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 取得(状態, &format!("/projects/{}/devices/new", p.id), &token).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{role} が登録画面に入れました"
        );

        // 送信しても拒否される。画面のボタンを隠すだけでは足りない
        let (状態, token) = 認証済み(db, &user).await;
        let (status, _) = 送信(
            状態,
            &format!("/projects/{}/devices", p.id),
            &token,
            &[("hostname", "sneaky-01")],
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{role} が登録できました");
    }

    assert_eq!(
        device::Entity::find().all(db).await.unwrap().len(),
        0,
        "機器が作られています"
    );
}

async fn operatorは登録できる(db: &DatabaseConnection) {
    let user = 利用者(db, "operator@example.com").await;
    let p = プロジェクト(db, "登録検証").await;
    メンバー(db, user.id, p.id, "Operator").await;
    let cfg = 構成(db, user.id, "operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, location, _) = 送信して行き先(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "srv-new"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg.to_string()),
            ("status", "provisioning"),
            ("power_watt", "350"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let created = device::Entity::find()
        .filter(device::Column::Hostname.eq("srv-new"))
        .one(db)
        .await
        .unwrap()
        .expect("機器が作られていません");

    // **登録後は機器の詳細へ移る。**このあとの手順（部品・インターフェース・SBOM）は
    // 詳細から始まる（設計書16.1）
    assert_eq!(
        location,
        format!("/projects/{}/devices/{}", p.id, created.id)
    );

    // 採番待ちでも登録できる（設計書23.2）
    assert!(created.asset_number.is_none());
    assert_eq!(created.uid.len(), 36);

    // **所在の割当が同時に作られること。**書かないとどの一覧にも出てこない
    let 割当 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(created.id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .expect("所在の割当がありません");
    assert_eq!(割当.location_type, "Project");
    assert_eq!(割当.location_id, Some(p.id));

    // お金の記録を何も入れなければ、購入の記録は作らない
    assert!(purchase::Entity::find().all(db).await.unwrap().is_empty());
}

/// **不正な入力は拒否すること**（設計書8.6、Q-21）。
///
/// 選択肢はサーバが描画しているため、語彙外の値が届くのは改竄か
/// クライアントの不具合しかありえない。**黙って別の値を保存すると、
/// どちらの場合も気付けない。**
async fn 不正な入力は拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "invalid@example.com").await;
    let p = プロジェクト(db, "検証").await;
    メンバー(db, user.id, p.id, "Operator").await;
    let cfg = 構成(db, user.id, "invalid").await.to_string();

    let 検証項目: &[(&str, &str, &str)] = &[
        ("device_type", "でたらめ", "種別の値が不正です"),
        ("status", "でたらめ", "状態の値が不正です"),
        // **予約は登録から作らない**（設計書11.6）
        ("status", "planned", "状態の値が不正です"),
        // 数値として読めない
        ("configuration_id", "abc", "構成の指定が不正です"),
        // **存在しないID。**確かめないと外部キー違反で500になる
        ("configuration_id", "9999", "構成の指定が不正です"),
        ("power_watt", "abc", "消費電力は0以上"),
        ("power_watt", "-1", "消費電力は0以上"),
    ];

    for (key, value, 期待) in 検証項目 {
        // **同じキーを2度送らない。**重複するとフォームの解釈自体が失敗し
        // （422）、検証を通ったのか弾かれたのかが判別できなくなる
        let mut fields: Vec<(&str, &str)> = vec![
            ("hostname", "invalid-01"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg),
            ("status", "provisioning"),
        ];
        match fields.iter_mut().find(|(k, _)| k == key) {
            Some(項目) => 項目.1 = value,
            None => fields.push((key, value)),
        }

        let (状態, token) = 認証済み(db, &user).await;
        let (status, body) = 送信(
            状態,
            &format!("/projects/{}/devices", p.id),
            &token,
            &fields,
        )
        .await;

        assert_eq!(
            status,
            StatusCode::OK,
            "{key}={value} が拒否されませんでした"
        );
        assert!(body.contains(期待), "{key}={value} のエラーが出ていません");
    }

    // 種別分類は構成を持たない機器でだけ見る（Physical では捨てる）
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "invalid-02"),
            ("device_type", "Virtual"),
            ("device_category", "でたらめ"),
            ("status", "provisioning"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("種別分類の値が不正です"));

    assert_eq!(
        device::Entity::find().all(db).await.unwrap().len(),
        0,
        "不正な入力で機器が作られています"
    );
}

/// 空欄が許される項目と、許されない項目を取り違えないこと。
///
/// 資産番号・消費電力は空でよい（資産番号は採番待ちがありうる、23.2）。
/// **仮想機器は構成もシリアル番号も持たない**（設計書6.2）ため、届いても捨てる。
async fn 空欄が許される項目は通ること(db: &DatabaseConnection) {
    let user = 利用者(db, "blank@example.com").await;
    let p = プロジェクト(db, "空欄検証").await;
    メンバー(db, user.id, p.id, "Operator").await;
    let cfg = 構成(db, user.id, "blank").await.to_string();

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "vm-blank"),
            ("device_type", "Virtual"),
            ("status", "running"),
            // **隠れている欄の値も送信される。**形態に合わないので捨てる
            ("configuration_id", &cfg),
            ("serial_number", "SN-HIDDEN"),
            ("device_category", "Server"),
            ("asset_number", ""),
            ("power_watt", ""),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER, "拒否されました: {body}");
    let created = device::Entity::find()
        .filter(device::Column::Hostname.eq("vm-blank"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created.power_watt, 0);
    assert!(
        created.configuration_id.is_none(),
        "仮想機器に構成が入っている"
    );
    assert!(
        created.serial_number.is_none(),
        "仮想機器にシリアル番号が入っている"
    );
    assert_eq!(created.device_category.as_deref(), Some("Server"));
}

/// **形態によって必須が変わること**（設計書16.1）。
///
/// 画面の `required` は常に見えている欄にしか付けられない（隠れた欄に付けると
/// フォーム全体が送信できなくなる）。形態ごとの必須はサーバーが確かめる。
async fn 形態によって必須が変わる(db: &DatabaseConnection) {
    let user = 利用者(db, "kind@example.com").await;
    let p = プロジェクト(db, "形態検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    for (device_type, 期待) in [
        ("Physical", "構成を選んでください"),
        ("Virtual", "種別分類を選んでください"),
        ("Logical", "種別分類を選んでください"),
    ] {
        let (状態, token) = 認証済み(db, &user).await;
        let (status, body) = 送信(
            状態,
            &format!("/projects/{}/devices", p.id),
            &token,
            &[
                ("hostname", "kind-01"),
                ("device_type", device_type),
                ("status", "provisioning"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{device_type} が通ってしまった");
        assert!(body.contains(期待), "{device_type} のエラーが出ていません");
    }
    assert!(device::Entity::find().all(db).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 登録時のお金の記録（設計書16.1、10.2）
// ---------------------------------------------------------------------------

/// **固定資産として管理しなくても、金額と発注番号が記録されること**（設計書16.1）。
async fn 固定資産でなくても購入は記録される(db: &DatabaseConnection) {
    let (user, p, cfg) = 登録の舞台(db, "purchase-only").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _, body) = 送信して行き先(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "po-01"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg.to_string()),
            ("status", "provisioning"),
            ("order_number", "PO-2026-118"),
            ("supplier", "〇〇商事"),
            ("acquisition_cost", "1,250,000"),
            ("acquisition_date", "2026-09-18"),
            // 管理しないので、送られてきても資産は作らない
            ("useful_life_years", "5"),
            ("depreciation_method", "straight_line"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let d = 登録された(db, "po-01").await;
    let 購入 = purchase::Entity::find()
        .one(db)
        .await
        .unwrap()
        .expect("購入の記録が無い");
    assert_eq!(購入.item_id, d.id);
    assert_eq!(購入.amount, 1_250_000);
    assert_eq!(購入.order_number.as_deref(), Some("PO-2026-118"));
    assert_eq!(購入.supplier.as_deref(), Some("〇〇商事"));
    assert!(fixed_asset::Entity::find()
        .all(db)
        .await
        .unwrap()
        .is_empty());
}

/// **固定資産として管理するなら、取得原価と取得日を複製して資産を作ること。**
/// 同じ値を2度入力させない（設計書16.1）。
async fn 固定資産として管理すると資産ができる(db: &DatabaseConnection) {
    let (user, p, cfg) = 登録の舞台(db, "asset").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, _, body) = 送信して行き先(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "fa-01"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg.to_string()),
            ("status", "provisioning"),
            ("acquisition_cost", "1200000"),
            ("acquisition_date", "2026-04-01"),
            ("manage_as_fixed_asset", "1"),
            ("useful_life_years", "5"),
            ("depreciation_method", "straight_line"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let d = 登録された(db, "fa-01").await;
    let 資産 = fixed_asset::Entity::find()
        .one(db)
        .await
        .unwrap()
        .expect("資産が無い");
    assert_eq!(資産.item_id, d.id);
    assert_eq!(資産.acquisition_cost, 1_200_000);
    assert_eq!(
        資産.acquisition_date,
        chrono::NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
    );
    assert_eq!(資産.useful_life_years, 5);
    // 購入の記録にも同じ値が入る
    let 購入 = purchase::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(購入.amount, 1_200_000);
}

/// **固定資産として管理するなら、取得原価と取得日は必須。**欠けていれば、
/// 機器も作らない（途中まで書かない）。
async fn 固定資産には取得日が要る(db: &DatabaseConnection) {
    let (user, p, cfg) = 登録の舞台(db, "asset-date").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "fa-02"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg.to_string()),
            ("status", "provisioning"),
            ("acquisition_cost", "1200000"),
            ("manage_as_fixed_asset", "1"),
            ("useful_life_years", "5"),
            ("depreciation_method", "straight_line"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("日付"), "取得日のエラーが出ていません");
    // **開いていた欄は開いたまま戻る**（チェックボックスが checked で描き直される）
    assert!(body
        .contains(r#"id="manage_as_fixed_asset" name="manage_as_fixed_asset" value="1" checked"#));
    assert!(device::Entity::find().all(db).await.unwrap().is_empty());
}

/// **保守契約は、既にある契約に足すことも、新しく作ることもできること**（設計書16.1）。
async fn 登録時に保守契約へ含められる(db: &DatabaseConnection) {
    let (user, p, cfg) = 登録の舞台(db, "contract").await;
    let v = ベンダー(db, "保守業者", user.id).await;

    // 新しい契約を作って足す
    let (状態, token) = 認証済み(db, &user).await;
    let (status, _, body) = 送信して行き先(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "mc-01"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg.to_string()),
            ("status", "provisioning"),
            ("register_maintenance", "1"),
            ("maintenance_mode", "new"),
            ("contract_number", "MC-2026-018"),
            ("contract_vendor_id", &v.to_string()),
            ("contract_start", "2026-04-01"),
            ("contract_end", "2029-03-31"),
            ("contract_amount", "240000"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let 契約 = maintenance_contract::Entity::find().all(db).await.unwrap();
    assert_eq!(契約.len(), 1);
    assert_eq!(契約[0].contract_number, "MC-2026-018");

    // 既にある契約に足す。**契約は増えず、品目だけが増える**
    let (状態, token) = 認証済み(db, &user).await;
    let (status, _, body) = 送信して行き先(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "mc-02"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg.to_string()),
            ("status", "provisioning"),
            ("register_maintenance", "1"),
            ("maintenance_mode", "existing"),
            ("maintenance_contract_id", &契約[0].id.to_string()),
            // 開いていない側の欄に値が残っていても、使わない
            ("contract_number", "MC-IGNORED"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        maintenance_contract::Entity::find()
            .all(db)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        maintenance_contract_item::Entity::find()
            .all(db)
            .await
            .unwrap()
            .len(),
        2
    );
}

/// **見えない契約には足せず、そのときは機器も作らないこと。**
async fn 見えない契約には足せない(db: &DatabaseConnection) {
    let (user, p, cfg) = 登録の舞台(db, "contract-hidden").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "mc-03"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg.to_string()),
            ("status", "provisioning"),
            ("register_maintenance", "1"),
            ("maintenance_mode", "existing"),
            ("maintenance_contract_id", "9999"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("契約を選び直してください"));
    assert!(device::Entity::find().all(db).await.unwrap().is_empty());
}

/// **「もう1台」は、形態・構成・状態を引き継いだ空のフォームへ戻すこと**（設計書16.1）。
/// ホスト名・シリアル番号・資産番号は引き継がない。
async fn もう1台は選択を引き継ぐ(db: &DatabaseConnection) {
    let (user, p, cfg) = 登録の舞台(db, "again").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, location, _) = 送信して行き先(
        状態,
        &format!("/projects/{}/devices", p.id),
        &token,
        &[
            ("hostname", "again-01"),
            ("device_type", "Physical"),
            ("configuration_id", &cfg.to_string()),
            ("serial_number", "SN-AGAIN-01"),
            ("status", "running"),
            ("then", "again"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let d = 登録された(db, "again-01").await;
    assert_eq!(
        location,
        format!("/projects/{}/devices/new?again={}", p.id, d.id)
    );

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, &location, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("again-01"), "直前の登録が知らされていない");
    assert!(
        body.contains(&format!(r#"<option value="{cfg}" selected>"#)),
        "構成が引き継がれていない"
    );
    assert!(
        body.contains(r#"<option value="running" selected>"#),
        "状態が引き継がれていない"
    );
    assert!(
        !body.contains("SN-AGAIN-01"),
        "シリアル番号まで引き継いでいる"
    );
    assert!(
        body.contains(r#"id="hostname" name="hostname" value="""#),
        "ホスト名が空になっていない"
    );
}

// ---------------------------------------------------------------------------
// 表示
// ---------------------------------------------------------------------------

/// 予約中の機器が区別表示されること（設計書11.6）。
async fn 予約中の機器は区別表示される(db: &DatabaseConnection) {
    let user = 利用者(db, "plan@example.com").await;
    let p = プロジェクト(db, "予約検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let d = 機器(db, "planned-01").await;
    let mut active: device::ActiveModel = d.clone().into();
    active.status = Set("planned".to_owned());
    active.update(db).await.unwrap();
    割当(db, d.id, p.id, Utc::now()).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;

    assert!(body.contains("planned-01"));
    assert!(body.contains("予約中"), "予約中の区別表示がありません");
}

/// **統合された機器は一覧に出ないこと**（設計書23.9）。
///
/// 重複として畳まれた側であり、出すと同じ機器が2行に見える。
/// 詳細を直接開いた場合は統合先へ誘導する。
async fn 統合された機器は一覧に出ない(db: &DatabaseConnection) {
    let user = 利用者(db, "merged@example.com").await;
    let p = プロジェクト(db, "統合検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let 残す = 機器(db, "keep-01").await;
    let 吸収 = 機器(db, "absorbed-01").await;
    割当(db, 残す.id, p.id, Utc::now()).await;
    割当(db, 吸収.id, p.id, Utc::now()).await;

    let mut active: device::ActiveModel = 吸収.clone().into();
    active.merged_into_device_id = Set(Some(残す.id));
    active.merged_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/devices?scope=all", p.id),
        &token,
    )
    .await;
    assert!(body.contains("keep-01"));
    assert!(!body.contains("absorbed-01"), "統合された機器が出ています");

    // 詳細は開けて、統合先へ誘導される
    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", p.id, 吸収.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("重複として統合されました"));
}

/// 所属するプロジェクトだけが一覧に出ること。
async fn 所属するプロジェクトだけ見える(db: &DatabaseConnection) {
    let user = 利用者(db, "list@example.com").await;
    let 自分の = プロジェクト(db, "所属している").await;
    let _ = プロジェクト(db, "所属していない").await;
    メンバー(db, user.id, 自分の.id, "Operator").await;
    メンバー(db, user.id, 自分の.id, "Approver").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 取得(状態, "/projects", &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("所属している"));
    assert!(!body.contains("所属していない"));
    // 兼務しているロールが両方出る（設計書5章）
    assert!(body.contains("Operator"));
    assert!(body.contains("Approver"));
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

/// **種別は日本語画面で訳して出し、保存する値は語彙のままであること**（#126）。
///
/// 略語（`VPN` 等）は訳さない。一覧・詳細・登録フォームの3画面を見る。
async fn 種別は表示だけを訳す(db: &DatabaseConnection) {
    let user = 利用者(db, "category-label@example.com").await;
    let p = プロジェクト(db, "種別表示").await;
    メンバー(db, user.id, p.id, "Operator").await;
    let d = 機器(db, "vsrv-01").await;
    割当(db, d.id, p.id, Utc::now()).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 一覧) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;
    assert!(一覧.contains("<td>サーバー</td>"), "一覧で訳されていません");

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 詳細) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", p.id, d.id),
        &token,
    )
    .await;
    assert!(詳細.contains("<dd>サーバー</dd>"), "詳細で訳されていません");

    let (状態, token) = 認証済み(db, &user).await;
    let (_, フォーム) = 取得(状態, &format!("/projects/{}/devices/new", p.id), &token).await;
    assert!(
        フォーム.contains(r#"<option value="Server" >サーバー</option>"#),
        "{フォーム}"
    );
    assert!(フォーム.contains(r#"<option value="VPN" >VPN</option>"#));
}

/// **一覧の状態にランプを添えること**（#163、設計書16.4）。
///
/// 文字だけだと、並んだ行の中で状態を見つけるのに目が要る。
async fn 一覧の状態にランプが付く(db: &DatabaseConnection) {
    let user = 利用者(db, "row-led@example.com").await;
    let p = プロジェクト(db, "ランプ検証").await;
    メンバー(db, user.id, p.id, "Viewer").await;
    let d = 機器(db, "led-01").await;
    割当(db, d.id, p.id, Utc::now()).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;
    assert!(body.contains(r#"<span class="led sm running">"#), "{body}");
    // ホスト名は等幅の列（0とOの判別が要る）
    assert!(body.contains(r#"<td class="mono">"#), "{body}");
}

/// **ホスト名を押して開けること**（#171）。右端の「詳細」は置かない。
async fn ホスト名を押して開ける(db: &DatabaseConnection) {
    let user = 利用者(db, "dev-link@example.com").await;
    let p = プロジェクト(db, "リンク検証").await;
    メンバー(db, user.id, p.id, "Viewer").await;
    let d = 機器(db, "link-01").await;
    割当(db, d.id, p.id, Utc::now()).await;

    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;
    assert!(
        body.contains(&format!(
            r#"<a href="/projects/{}/devices/{}">link-01</a>"#,
            p.id, d.id
        )),
        "{body}"
    );
    let 本文 = &body[body.find(r#"<main class="content">"#).unwrap()..];
    assert!(!本文.contains(">詳細<"), "「詳細」が残っています: {本文}");
}

/// **見出しで並べ替えられ、並びがURLに残ること**（#171）。
async fn 見出しで並べ替えられる(db: &DatabaseConnection) {
    let user = 利用者(db, "dev-sort@example.com").await;
    let p = プロジェクト(db, "並べ替え検証").await;
    メンバー(db, user.id, p.id, "Viewer").await;
    for name in ["srv-c", "srv-a", "srv-b"] {
        let d = 機器(db, name).await;
        割当(db, d.id, p.id, Utc::now()).await;
    }

    // 既定はホスト名の昇順
    let (状態, token) = 認証済み(db, &user).await;
    let (_, body) = 取得(状態, &format!("/projects/{}/devices", p.id), &token).await;
    assert!(
        body.find("srv-a").unwrap() < body.find("srv-c").unwrap(),
        "既定が昇順になっていません"
    );
    // 見出しのリンクは、押すと降順になる
    // 属性の中の & は実体参照で出る
    assert!(body.contains("sort=hostname"), "{body}");
    assert!(body.contains("dir=desc"), "{body}");

    let (状態, token) = 認証済み(db, &user).await;
    let (_, 降順) = 取得(
        状態,
        &format!("/projects/{}/devices?sort=hostname&dir=desc", p.id),
        &token,
    )
    .await;
    assert!(
        降順.find("srv-c").unwrap() < 降順.find("srv-a").unwrap(),
        "降順になっていません"
    );
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

/// 送信して、状態・リダイレクト先・本文を返す。
async fn 送信して行き先(
    state: AppState,
    uri: &str,
    token: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, String, String) {
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
    let location = res
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let (status, body) = 分解(res).await;
    (status, location, body)
}

async fn 分解(res: Response<Body>) -> (StatusCode, String) {
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn ベンダー(db: &DatabaseConnection, name: &str, user_id: i32) -> i32 {
    vendor::ActiveModel {
        name: Set(name.to_owned()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

/// 共有カタログの構成を1つ作り、そのIDを返す。**Physical の登録には構成が要る。**
async fn 構成(db: &DatabaseConnection, user_id: i32, name: &str) -> i32 {
    let v = ベンダー(db, &format!("ベンダー-{name}"), user_id).await;
    let m = chassis_model::ActiveModel {
        vendor_id: Set(v),
        model_name: Set(format!("型-{name}")),
        device_category: Set("Server".to_owned()),
        height_u: Set(1),
        mount_form: Set("RackU".to_owned()),
        rack_width: Set(Some("Full".to_owned())),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    configuration::ActiveModel {
        chassis_model_id: Set(m.id),
        name: Set(format!("構成-{name}")),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

/// 登録を試す場。Operator の利用者、プロジェクト、構成。
async fn 登録の舞台(
    db: &DatabaseConnection,
    name: &str,
) -> (app_user::Model, project::Model, i32) {
    let user = 利用者(db, &format!("{name}@example.com")).await;
    let p = プロジェクト(db, &format!("登録-{name}")).await;
    メンバー(db, user.id, p.id, "Operator").await;
    let cfg = 構成(db, user.id, name).await;
    (user, p, cfg)
}

async fn 登録された(db: &DatabaseConnection, hostname: &str) -> device::Model {
    device::Entity::find()
        .filter(device::Column::Hostname.eq(hostname))
        .one(db)
        .await
        .unwrap()
        .expect("機器が作られていません")
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証".to_owned()),
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
        user_id: Set(user_id),
        project_id: Set(project_id),
        role: Set(role.to_owned()),
        admin_rank: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 機器(db: &DatabaseConnection, hostname: &str) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        external_id: Set(None),
        merged_into_device_id: Set(None),
        merged_at: Set(None),
        configuration_id: Set(None),
        device_type: Set("Physical".to_owned()),
        device_category: Set(Some("Server".to_owned())),
        hostname: Set(hostname.to_owned()),
        serial_number: Set(None),
        asset_number: Set(None),
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

async fn 割当(
    db: &DatabaseConnection,
    device_id: i32,
    project_id: i32,
    from: chrono::DateTime<Utc>,
) -> device_assignment::Model {
    device_assignment::ActiveModel {
        device_id: Set(device_id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(project_id)),
        work_order_id: Set(None),
        from_date: Set(from),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 閉じる(db: &DatabaseConnection, row: device_assignment::Model) {
    let mut active: device_assignment::ActiveModel = row.into();
    active.to_date = Set(Some(Utc::now()));
    active.update(db).await.unwrap();
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 過去に所属した機器も閲覧できる);
        全検証!(@one $用意, $属性, 無関係なプロジェクトからは見えない);
        全検証!(@one $用意, $属性, 非メンバーは入れない);
        全検証!(@one $用意, $属性, 閲覧のみのロールは登録できない);
        全検証!(@one $用意, $属性, operatorは登録できる);
        全検証!(@one $用意, $属性, 不正な入力は拒否される);
        全検証!(@one $用意, $属性, 空欄が許される項目は通ること);
        全検証!(@one $用意, $属性, 形態によって必須が変わる);
        全検証!(@one $用意, $属性, 固定資産でなくても購入は記録される);
        全検証!(@one $用意, $属性, 固定資産として管理すると資産ができる);
        全検証!(@one $用意, $属性, 固定資産には取得日が要る);
        全検証!(@one $用意, $属性, 登録時に保守契約へ含められる);
        全検証!(@one $用意, $属性, 見えない契約には足せない);
        全検証!(@one $用意, $属性, もう1台は選択を引き継ぐ);
        全検証!(@one $用意, $属性, 予約中の機器は区別表示される);
        全検証!(@one $用意, $属性, 統合された機器は一覧に出ない);
        全検証!(@one $用意, $属性, 所属するプロジェクトだけ見える);
        全検証!(@one $用意, $属性, 種別は表示だけを訳す);
        全検証!(@one $用意, $属性, 一覧の状態にランプが付く);
        全検証!(@one $用意, $属性, ホスト名を押して開ける);
        全検証!(@one $用意, $属性, 見出しで並べ替えられる);
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

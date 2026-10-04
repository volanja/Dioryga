//! インスタンスCSV取込の結合テスト（設計書23.2、23.5、23.6、23.9）。
//!
//! **23章で最も壊れやすいのは突合である。**取り違えると、同じ機器が二重に
//! 登録されるか、別の機器を上書きする。どちらも事後の修復が難しいので、
//! 解決順序の各段と重複検出をここで固定する。

use chrono::{Duration, Utc};
use dioryga::import::{instances, Outcome};
use entity::{app_user, device, device_assignment, project, warehouse};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

const 見出し: &str =
    "uid,external_id,hostname,serial_number,asset_number,device_type,power_watt,status\n";

fn csv(rows: &[&str]) -> String {
    format!("{見出し}{}", rows.join("\n"))
}

// ---------------------------------------------------------------------------
// 突合の解決順序（設計書23.2）
// ---------------------------------------------------------------------------

/// **故障は `health` の列に書くこと**（#221、設計書6.3）。
///
/// 旧い `failed` / `repairing` を `status` に書いたファイルはエラーにする。
/// 黙って `health` へ読み替えると、ファイルの誤りに気付けない。空欄は `ok`。
async fn 故障はhealthの列に書く(db: &DatabaseConnection) {
    let p = プロジェクト(db, "故障の列").await;
    let 列 = format!("{},health\n", 見出し.trim_end());

    for 旧い値 in ["failed", "repairing"] {
        let rows = instances::parse_devices(&format!(
            "{列},,old-{旧い値},SN-OLD-{旧い値},,Physical,400,{旧い値},"
        ))
        .unwrap();
        let report = instances::dry_run(db, p.id, &rows, &[]).await.unwrap();
        assert_eq!(report.count(Outcome::Error), 1, "{旧い値}: {report}");
    }
    let 語彙外 =
        instances::parse_devices(&format!("{列},,bad01,SN-BAD,,Physical,400,running,broken"))
            .unwrap();
    let report = instances::dry_run(db, p.id, &語彙外, &[]).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");

    let match_on = ["serial_number".to_owned()];
    let rows = instances::parse_devices(&format!(
        "{列},,web01,SN-H1,,Physical,400,running,failed\n,,web02,SN-H2,,Physical,400,standby,"
    ))
    .unwrap();
    instances::apply(db, p.id, &rows, &match_on, Utc::now(), 1)
        .await
        .unwrap();
    for (serial, status, health) in [("SN-H1", "running", "failed"), ("SN-H2", "standby", "ok")] {
        let d = device::Entity::find()
            .filter(device::Column::SerialNumber.eq(serial))
            .one(db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((d.status.as_str(), d.health.as_str()), (status, health));
    }

    // health だけを変えても更新として数える
    let 直す = instances::parse_devices(&format!(
        "{列},,web01,SN-H1,,Physical,400,running,ok\n,,web02,SN-H2,,Physical,400,standby,"
    ))
    .unwrap();
    let report = instances::dry_run(db, p.id, &直す, &match_on)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Updated), 1, "{report}");
    assert_eq!(report.count(Outcome::Unchanged), 1, "{report}");
}

/// **倉庫プロジェクトでは、既存の機器の `status` を動かさないこと**（#221、設計書6.3）。
///
/// 倉庫にある間の `status` には意味が無い。比べると、空欄の既定（構築中）と
/// 食い違って毎回「更新」になる。**黙って捨てず**、書いてあれば行の説明に添える。
async fn 倉庫プロジェクトではstatusを動かさない(db: &DatabaseConnection) {
    let p = プロジェクト(db, "倉庫の取込").await;
    crate::support::倉庫プロジェクトにする(db, p.id).await;
    let match_on = ["serial_number".to_owned()];

    let 初回 = instances::parse_devices(&csv(&[",,spare01,SN-WH,,Physical,400,running"])).unwrap();
    instances::apply(db, p.id, &初回, &match_on, Utc::now(), 1)
        .await
        .unwrap();

    for 値 in ["", "standby"] {
        let rows =
            instances::parse_devices(&csv(&[&format!(",,spare01,SN-WH,,Physical,400,{値}")]))
                .unwrap();
        let report = instances::dry_run(db, p.id, &rows, &match_on)
            .await
            .unwrap();
        assert_eq!(report.count(Outcome::Unchanged), 1, "{値}: {report}");
        let 説明あり = report.entries[0]
            .detail
            .contains("倉庫プロジェクトでは status を反映しません");
        assert_eq!(説明あり, !値.is_empty(), "{値}: {report}");
        instances::apply(db, p.id, &rows, &match_on, Utc::now(), 2)
            .await
            .unwrap();
    }
    let d = device::Entity::find()
        .filter(device::Column::SerialNumber.eq("SN-WH"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.status, "running");
}

/// **① uid で特定できること。**export → 編集 → import の往復の要。
async fn uidで突合する(db: &DatabaseConnection) {
    let p = プロジェクト(db, "突合検証").await;
    let 既存 = 機器(db, p.id, "old-name", Some("SN-1"), None).await;

    let rows = instances::parse_devices(&csv(&[&format!(
        "{},,new-name,SN-1,,Physical,400,running",
        既存.uid
    )]))
    .unwrap();

    let report = instances::dry_run(db, p.id, &rows, &["serial_number".to_owned()])
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Updated), 1, "{report}");

    instances::apply(db, p.id, &rows, &[], Utc::now(), 1)
        .await
        .unwrap();

    let 後 = device::Entity::find_by_id(既存.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(後.hostname, "new-name");
    assert_eq!(台数(db).await, 1, "新しい機器が作られています");
}

/// **uid が指定されているのに見つからなければエラー。**
///
/// 書き出したファイルを編集して戻す運用が前提なので、uid は存在するはず。
/// 黙って新規作成すると、消えたはずの機器が増える。
async fn 存在しないuidはエラー(db: &DatabaseConnection) {
    let p = プロジェクト(db, "uid検証").await;
    let rows = instances::parse_devices(&csv(&[
        "11111111-1111-1111-1111-111111111111,,web01,,,Physical,,",
    ]))
    .unwrap();

    let report = instances::dry_run(db, p.id, &rows, &[]).await.unwrap();
    assert!(report.has_error());
    assert!(report.errors().any(|e| e.detail.contains("見つかりません")));
}

/// ② external_id で突合できること。
async fn external_idで突合する(db: &DatabaseConnection) {
    let p = プロジェクト(db, "external検証").await;
    let 既存 = 機器(db, p.id, "web01", None, Some("SV-0001")).await;

    let rows = instances::parse_devices(&csv(&[",SV-0001,web01-renamed,,,Physical,,"])).unwrap();
    instances::apply(db, p.id, &rows, &[], Utc::now(), 1)
        .await
        .unwrap();

    let 後 = device::Entity::find_by_id(既存.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(後.hostname, "web01-renamed");
    assert_eq!(台数(db).await, 1);
}

/// ③ match_on で突合できること。初回取込の次に流す典型。
async fn match_onで突合する(db: &DatabaseConnection) {
    let p = プロジェクト(db, "matchon検証").await;
    let 既存 = 機器(db, p.id, "web01", Some("JP123"), None).await;

    let rows = instances::parse_devices(&csv(&[",,web01,JP123,A-001,Physical,,"])).unwrap();
    instances::apply(
        db,
        p.id,
        &rows,
        &["serial_number".to_owned()],
        Utc::now(),
        1,
    )
    .await
    .unwrap();

    let 後 = device::Entity::find_by_id(既存.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(後.asset_number.as_deref(), Some("A-001"));
    assert_eq!(台数(db).await, 1);
}

/// ④ 該当が無ければ新規作成し、**uid を採番すること**（設計書23.2）。
///
/// 採番されないと、書き出して往復する運用が始められない。
async fn 新規作成でuidが採番される(db: &DatabaseConnection) {
    let p = プロジェクト(db, "採番検証").await;
    let rows = instances::parse_devices(&csv(&[",,web01,JP999,,Physical,450,running"])).unwrap();

    instances::apply(
        db,
        p.id,
        &rows,
        &["serial_number".to_owned()],
        Utc::now(),
        1,
    )
    .await
    .unwrap();

    let 作成 = device::Entity::find()
        .filter(device::Column::Hostname.eq("web01"))
        .one(db)
        .await
        .unwrap()
        .expect("機器が作られていません");

    assert_eq!(作成.uid.len(), 36, "uidが採番されていません");
    assert_eq!(作成.power_watt, 450);

    // **所在の割当も作られること。**書かないとどの一覧にも出てこない
    let 割当 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(作成.id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .expect("所在の割当がありません");
    assert_eq!(割当.location_id, Some(p.id));
}

// ---------------------------------------------------------------------------
// 重複検出（設計書23.9.1）
// ---------------------------------------------------------------------------

/// **黙って2台目を作らないこと**（設計書23.9.1）。
///
/// 突合できなかったのに識別子が既存機器と一致する場合はエラーとする。
/// 利用者はドライランの指摘を見て、uid を書くか別機器だと確認できる。
async fn 突合できないのに識別子が一致すればエラー(db: &DatabaseConnection) {
    let p = プロジェクト(db, "重複検証").await;
    let _ = 機器(db, p.id, "web01", Some("JP123"), None).await;

    // match_on を asset_number にしているので突合できないが、
    // serial_number と hostname が既存と一致する
    let rows = instances::parse_devices(&csv(&[",,web01,JP123,,Physical,,"])).unwrap();
    let report = instances::dry_run(db, p.id, &rows, &["asset_number".to_owned()])
        .await
        .unwrap();

    assert!(report.has_error(), "重複が見逃されています");
    assert!(report
        .errors()
        .any(|e| e.detail.contains("uid か external_id を指定")));

    // 反映も止まる
    assert!(
        instances::apply(db, p.id, &rows, &["asset_number".to_owned()], Utc::now(), 1)
            .await
            .is_err()
    );
    assert_eq!(台数(db).await, 1, "2台目が作られています");
}

/// **統合された機器で取り込んでも統合先に着地すること**（設計書23.9.4）。
///
/// 統合後も従来の取込ファイルがそのまま使える、というのがリダイレクト方式の実利。
async fn 統合された機器は統合先に着地する(db: &DatabaseConnection) {
    let p = プロジェクト(db, "統合検証").await;
    let 残す = 機器(db, p.id, "keep", Some("SN-KEEP"), None).await;
    let 吸収 = 機器(db, p.id, "absorbed", Some("SN-OLD"), None).await;

    let mut active: device::ActiveModel = 吸収.clone().into();
    active.merged_into_device_id = Set(Some(残す.id));
    active.merged_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();

    // 吸収された側の uid で取り込む
    let rows = instances::parse_devices(&csv(&[&format!(
        "{},,renamed-by-import,SN-KEEP,,Physical,,",
        吸収.uid
    )]))
    .unwrap();
    instances::apply(db, p.id, &rows, &[], Utc::now(), 1)
        .await
        .unwrap();

    let 統合先 = device::Entity::find_by_id(残す.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        統合先.hostname, "renamed-by-import",
        "統合先に着地していません"
    );

    let 吸収後 = device::Entity::find_by_id(吸収.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        吸収後.hostname, "absorbed",
        "吸収された側が書き換わりました"
    );
    assert_eq!(台数(db).await, 2);
}

/// ファイル内で識別子が重複していればエラーになること。
///
/// **DBと突き合わせる前に弾く。**弾かないと、同じ機器を2回作るか、
/// 2行目が1行目を上書きする。
async fn ファイル内の重複はエラー(db: &DatabaseConnection) {
    let p = プロジェクト(db, "行内重複検証").await;
    let rows = instances::parse_devices(&csv(&[
        ",SV-0001,web01,,,Physical,,",
        ",SV-0001,web02,,,Physical,,",
    ]))
    .unwrap();

    let report = instances::dry_run(db, p.id, &rows, &[]).await.unwrap();
    assert!(report.has_error());
    assert!(report.errors().any(|e| e.detail.contains("重複しています")));

    // **1行につき判定は1つ**（#108）。重複した行を「新規」としても数えると、
    // ドライランの件数が行数と合わなくなる（23.6）
    let p = プロジェクト(db, "行内重複の件数").await;
    let rows = instances::parse_devices(&csv(&[
        ",EX-1,web01,SN-1,,Physical,,",
        ",EX-1,web02,SN-1,,Physical,,",
    ]))
    .unwrap();

    let report = instances::dry_run(db, p.id, &rows, &[]).await.unwrap();
    assert_eq!(
        report.entries.len(),
        rows.len(),
        "レポートの項目数が行数と一致しません: {:?}",
        report.entries
    );
    assert_eq!(
        report.count(Outcome::Error),
        1,
        "識別子が2つ重複しても、エラーは1件のはずです"
    );
    assert_eq!(
        report.count(Outcome::Created),
        1,
        "重複した行が新規として数えられています"
    );

    let エラー = report.errors().next().unwrap();
    assert!(
        エラー.detail.contains("external_id「EX-1」")
            && エラー.detail.contains("serial_number「SN-1」"),
        "重複した識別子がまとめて示されていません: {}",
        エラー.detail
    );
}

// ---------------------------------------------------------------------------
// 宣言的であること（設計書23.1）
// ---------------------------------------------------------------------------

/// **同じファイルを2回流しても結果が変わらないこと**（設計書23.1）。
///
/// 命令形にすると履歴行が重複して事実が壊れる。「1行直して再取込」という
/// 運用が成立するかは、ここで決まる。
async fn 二度流しても履歴が増えない(db: &DatabaseConnection) {
    let p = プロジェクト(db, "べき等検証").await;
    let rows = instances::parse_devices(&csv(&[",,web01,JP123,,Physical,450,running"])).unwrap();
    let match_on = ["serial_number".to_owned()];

    instances::apply(db, p.id, &rows, &match_on, Utc::now(), 1)
        .await
        .unwrap();
    let 一回目 = 割当の件数(db).await;

    let 二回目のレポート = instances::apply(db, p.id, &rows, &match_on, Utc::now(), 2)
        .await
        .unwrap();

    assert_eq!(台数(db).await, 1);
    assert_eq!(割当の件数(db).await, 一回目, "履歴行が増えています");
    // **現在の事実と一致していれば履歴行を作らない**
    assert_eq!(二回目のレポート.count(Outcome::Unchanged), 1);
    assert_eq!(二回目のレポート.count(Outcome::Created), 0);
}

/// **今、他のプロジェクトにある機器はエラーにし、引き込まないこと**（#143）。
///
/// 黙って引き込むと、チケットを通らない移譲になる（11章）。`uid` はプロジェクト
/// の外も探すため、書き出したファイルを別のプロジェクトで流すと起きる。
/// 過去にこのプロジェクトにあって移譲された機器も、`match_on` の候補に入るため
/// 同じ扱いにする。
async fn 他のプロジェクトにある機器はエラー(db: &DatabaseConnection) {
    let 元 = プロジェクト(db, "移譲元").await;
    let 先 = プロジェクト(db, "移譲先").await;
    let d = 機器(db, 元.id, "moving", Some("SN-MV"), None).await;

    // 書き出したファイルのように、uid で書いた行
    let uidで =
        instances::parse_devices(&csv(&[&format!("{},,moving,SN-MV,,Physical,,", d.uid)])).unwrap();
    // 移譲した後に、移譲元で普段のCSVを流し直す
    所属を移す(db, d.id, "Project", Some(先.id)).await;
    let 再取込 = instances::parse_devices(&csv(&[",,moving,SN-MV,,Physical,,"])).unwrap();

    for rows in [&再取込, &uidで] {
        let report = instances::dry_run(db, 元.id, rows, &["serial_number".to_owned()])
            .await
            .unwrap();
        assert_eq!(report.count(Outcome::Error), 1, "{report}");
        assert!(
            report.errors().any(|e| e.detail.contains("Transfer")),
            "移譲の案内がありません: {report}"
        );
        assert!(
            instances::apply(
                db,
                元.id,
                rows,
                &["serial_number".to_owned()],
                Utc::now(),
                1
            )
            .await
            .is_err(),
            "反映できてしまいます"
        );
    }

    // 今ある移譲先からは触れる
    let report = instances::dry_run(db, 先.id, &uidで, &[]).await.unwrap();
    assert!(!report.has_error(), "{report}");

    // 移譲先とは別のプロジェクトから uid で書いても引き込めない
    let 第三 = プロジェクト(db, "無関係").await;
    let report = instances::dry_run(db, 第三.id, &uidで, &[]).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");

    let 履歴 = 所属の履歴(db, d.id).await;
    assert_eq!(履歴.len(), 2, "所属の履歴が変わっています");
    let 現在: Vec<_> = 履歴.iter().filter(|a| a.to_date.is_none()).collect();
    assert_eq!(現在.len(), 1);
    assert_eq!(現在[0].location_id, Some(先.id));
}

/// **既存の機器の所属は付け替えないこと**（#143）。
///
/// 倉庫へ移した機器を普段の機器のCSVに書いたまま流し直しても、プロジェクトへ
/// 戻さない。戻すと、流すたびに所属の履歴が増え（23.1）、倉庫からの払い出しが
/// チケットなしに起きる。廃棄した機器も同じ。**機器の値の更新は通す。**
async fn 倉庫や廃棄にある機器の所属は付け替えない(db: &DatabaseConnection) {
    let p = プロジェクト(db, "付け替え検証").await;
    let 倉庫 = 倉庫(db, "付け替え検証倉庫").await;
    let 置いた = 機器(db, p.id, "spare01", Some("SN-SP"), None).await;
    let 捨てた = 機器(db, p.id, "old01", Some("SN-OLD"), None).await;
    所属を移す(db, 置いた.id, "Warehouse", Some(倉庫.id)).await;
    所属を移す(db, 捨てた.id, "Disposed", None).await;

    let match_on = ["serial_number".to_owned()];
    let 同じ値 = instances::parse_devices(&csv(&[
        ",,spare01,SN-SP,,Physical,400,running",
        ",,old01,SN-OLD,,Physical,400,running",
    ]))
    .unwrap();
    let report = instances::dry_run(db, p.id, &同じ値, &match_on)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Unchanged), 2, "{report}");

    // 値が変われば機器は更新するが、所属には触れない
    let 値を変える = instances::parse_devices(&csv(&[
        ",,spare01,SN-SP,,Physical,400,standby",
        ",,old01,SN-OLD,,Physical,400,standby",
    ]))
    .unwrap();
    for _ in 0..2 {
        instances::apply(db, p.id, &値を変える, &match_on, Utc::now(), 1)
            .await
            .unwrap();
    }

    for (d, 種別) in [(&置いた, "Warehouse"), (&捨てた, "Disposed")] {
        let 後 = device::Entity::find_by_id(d.id)
            .one(db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(後.status, "standby");
        let 履歴 = 所属の履歴(db, d.id).await;
        assert_eq!(履歴.len(), 2, "{種別}: 所属の履歴が増えています");
        let 現在: Vec<_> = 履歴.iter().filter(|a| a.to_date.is_none()).collect();
        assert_eq!(現在.len(), 1);
        assert_eq!(現在[0].location_type, 種別);
    }
}

/// **`as_of` が履歴の `from_date` に使われること**（設計書23.1）。
///
/// 過去のデータを遡って登録する場合に効く。実行時刻で書くと、
/// 「いつからそうだったか」が取込作業日にすり替わる。
async fn as_ofが履歴の開始日になる(db: &DatabaseConnection) {
    let p = プロジェクト(db, "as_of検証").await;
    let 基準 = Utc::now() - Duration::days(365);

    let rows = instances::parse_devices(&csv(&[",,old-server,SN-OLD,,Physical,,"])).unwrap();
    instances::apply(db, p.id, &rows, &["serial_number".to_owned()], 基準, 1)
        .await
        .unwrap();

    let 割当 = device_assignment::Entity::find()
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .unwrap();

    let 差 = (割当.from_date - 基準).num_seconds().abs();
    assert!(差 < 2, "from_date に as_of が使われていません");
}

/// 語彙外の値は拒否すること（Q-21と同じ判断）。
async fn 語彙外の値は拒否される(db: &DatabaseConnection) {
    let p = プロジェクト(db, "語彙検証").await;
    let rows = instances::parse_devices(&csv(&[",,web01,,,でたらめ,,"])).unwrap();

    let report = instances::dry_run(db, p.id, &rows, &[]).await.unwrap();
    assert!(report.has_error());
    assert!(report.errors().any(|e| e.detail.contains("device_type")));
}

/// **種別の略語は大文字だけを受け、空欄は許すこと**（設計書8.6、#125）。
///
/// 構成を持たない機器（仮想アプライアンス等）だけが種別を持つため、空欄が普通である。
async fn 種別の旧表記は拒否される(db: &DatabaseConnection) {
    let p = プロジェクト(db, "種別検証").await;
    let 種別つき = |category: &str| {
        format!(
            "uid,external_id,hostname,serial_number,asset_number,device_type,device_category,power_watt,status\n\
             ,,vfw01,,,Virtual,{category},,"
        )
    };

    let rows = instances::parse_devices(&種別つき("Vpn")).unwrap();
    let report = instances::dry_run(db, p.id, &rows, &[]).await.unwrap();
    assert!(
        report.errors().any(|e| e.detail.contains("「Vpn」")),
        "{report}"
    );

    for ok in ["VPN", ""] {
        let rows = instances::parse_devices(&種別つき(ok)).unwrap();
        let report = instances::dry_run(db, p.id, &rows, &[]).await.unwrap();
        assert!(!report.has_error(), "「{ok}」: {report}");
    }
}

// ---------------------------------------------------------------------------
// プロジェクトの解決（設計書5.3）
// ---------------------------------------------------------------------------

/// `uid` → `code` → `name` の順で解決し、**同名が複数あればエラー**であること。
async fn プロジェクトの解決順序(db: &DatabaseConnection) {
    let a = プロジェクト(db, "同名の案件").await;
    let mut active: entity::project::ActiveModel = a.clone().into();
    active.code = Set(Some("PRJ-A".to_owned()));
    let a = active.update(db).await.unwrap();
    let _ = プロジェクト(db, "同名の案件").await;

    // uid で引ける
    let 結果 = instances::解決するプロジェクト(db, &a.uid).await.unwrap();
    assert_eq!(結果.unwrap().id, a.id);

    // code で引ける
    let 結果 = instances::解決するプロジェクト(db, "PRJ-A").await.unwrap();
    assert_eq!(結果.unwrap().id, a.id);

    // name が重複していればエラー
    let 結果 = instances::解決するプロジェクト(db, "同名の案件")
        .await
        .unwrap();
    assert!(結果.is_err());
    assert!(結果.unwrap_err().contains("uid か code で指定"));
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 台数(db: &DatabaseConnection) -> usize {
    device::Entity::find().all(db).await.unwrap().len()
}

async fn 割当の件数(db: &DatabaseConnection) -> usize {
    device_assignment::Entity::find()
        .all(db)
        .await
        .unwrap()
        .len()
}

async fn 所属の履歴(db: &DatabaseConnection, device_id: i32) -> Vec<device_assignment::Model> {
    device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .all(db)
        .await
        .unwrap()
}

/// 現在の所属を閉じ、別の所属を開く。移譲や入庫を済ませた状態を作る。
async fn 所属を移す(
    db: &DatabaseConnection,
    device_id: i32,
    location_type: &str,
    location_id: Option<i32>,
) {
    let 現在 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let 時刻 = Utc::now() - Duration::days(1);
    let mut active: device_assignment::ActiveModel = 現在.into();
    active.to_date = Set(Some(時刻));
    active.update(db).await.unwrap();

    device_assignment::ActiveModel {
        device_id: Set(device_id),
        location_type: Set(location_type.to_owned()),
        location_id: Set(location_id),
        work_order_id: Set(None),
        from_date: Set(時刻),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn 倉庫(db: &DatabaseConnection, name: &str) -> warehouse::Model {
    let u = 利用者(db, &format!("{}@example.com", uuid::Uuid::new_v4())).await;
    warehouse::ActiveModel {
        name: Set(name.to_owned()),
        address: Set(String::new()),
        retired_at: Set(None),
        created_by: Set(u.id),
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

/// 機器を作り、そのプロジェクトへの割当も作る。
async fn 機器(
    db: &DatabaseConnection,
    project_id: i32,
    hostname: &str,
    serial: Option<&str>,
    external_id: Option<&str>,
) -> device::Model {
    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        external_id: Set(external_id.map(str::to_owned)),
        merged_into_device_id: Set(None),
        merged_at: Set(None),
        configuration_id: Set(None),
        device_type: Set("Physical".to_owned()),
        device_category: Set(None),
        hostname: Set(hostname.to_owned()),
        serial_number: Set(serial.map(str::to_owned)),
        asset_number: Set(None),
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
        location_id: Set(Some(project_id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(30)),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    d
}

/// 倉庫の作成者に使う。
async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("取込".to_owned()),
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

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, uidで突合する);
        全検証!(@one $用意, $属性, 故障はhealthの列に書く);
        全検証!(@one $用意, $属性, 倉庫プロジェクトではstatusを動かさない);
        全検証!(@one $用意, $属性, 存在しないuidはエラー);
        全検証!(@one $用意, $属性, external_idで突合する);
        全検証!(@one $用意, $属性, match_onで突合する);
        全検証!(@one $用意, $属性, 新規作成でuidが採番される);
        全検証!(@one $用意, $属性, 突合できないのに識別子が一致すればエラー);
        全検証!(@one $用意, $属性, 統合された機器は統合先に着地する);
        全検証!(@one $用意, $属性, ファイル内の重複はエラー);
        全検証!(@one $用意, $属性, 二度流しても履歴が増えない);
        全検証!(@one $用意, $属性, 他のプロジェクトにある機器はエラー);
        全検証!(@one $用意, $属性, 倉庫や廃棄にある機器の所属は付け替えない);
        全検証!(@one $用意, $属性, as_ofが履歴の開始日になる);
        全検証!(@one $用意, $属性, 語彙外の値は拒否される);
        全検証!(@one $用意, $属性, 種別の旧表記は拒否される);
        全検証!(@one $用意, $属性, プロジェクトの解決順序);
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

//! 組織データの取込の結合テスト（設計書23.8、#130）。
//!
//! 利用者・倉庫・プロジェクト・メンバーを、System Admin が1つのマニフェストで
//! 流す。issue の確認観点をそのまま並べている。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::import::run::{self, RunError};
use dioryga::import::{ImportError, Outcome};
use dioryga::server::{router, AppState};
use entity::{app_setting, app_user, audit_log, import_run, project, project_member, warehouse};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};
use tower::ServiceExt;

const 利用者の見出し: &str = "username,name,email,locale,status\n";
const メンバーの見出し: &str = "project,username,role,admin_rank,remove\n";

/// **System Admin 以外が流すと拒否されること**（23.8、3章の境界）。
async fn system_admin以外は流せない(db: &DatabaseConnection) {
    let _ = 管理者(db).await;
    let operator = 利用者(db, "hotaka", false).await;
    let p = プロジェクト(db, "KATAKURI-01").await;
    所属(db, operator.id, p.id, "Operator", None).await;

    let dir = 取込ファイル(&[(
        "user",
        "users.csv",
        &format!("{利用者の見出し}tateyama,立山 滝見,,,\n"),
    )]);
    let 結果 = run::run(db, &dir.join("manifest.yaml"), "hotaka", false).await;

    assert!(
        matches!(結果, Err(RunError::NotSystemAdmin)),
        "{:?}",
        結果.err()
    );
}

/// 利用者・倉庫・プロジェクト・メンバーを1回で取り込めること。
///
/// **新しい利用者はパスワード未設定で、System Admin にならない**（23.8）。
async fn 組織データを取り込める(db: &DatabaseConnection) {
    let admin = 管理者(db).await;
    let dir = 取込ファイル(&[
        (
            "project_member",
            "members.csv",
            &format!("{メンバーの見出し}KATAKURI-01,yarigatake,Administrator,Primary,\nKATAKURI-01,hotaka,Operator,,\n"),
        ),
        ("project", "projects.csv", "uid,code,name,description,currency\n,KATAKURI-01,カタクリ基盤更改,,USD\n"),
        ("warehouse", "warehouses.csv", "name,address\n白馬倉庫,第1棟\n"),
        (
            "user",
            "users.csv",
            &format!("{利用者の見出し}Yarigatake,槍ヶ岳 大川,yarigatake@example.invalid,,\nhotaka,穂高 古城,,en,\n"),
        ),
    ]);

    let 実行 = run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();
    assert!(!実行.report.has_error(), "{}", 詳細(&実行.report));
    assert_eq!(
        実行.report.count(Outcome::Created),
        6,
        "{}",
        詳細(&実行.report)
    );

    let yari = 利用者を引く(db, "yarigatake")
        .await
        .expect("小文字で保存されていません");
    assert_eq!(yari.password_hash, "", "パスワードが設定されています");
    // 変更の強制はリセットが立てる。空のハッシュのままではログインできない（23.8）
    assert!(!yari.must_change_password);
    assert!(!yari.is_system_admin);
    assert_eq!(yari.email.as_deref(), Some("yarigatake@example.invalid"));
    assert_eq!(利用者を引く(db, "hotaka").await.unwrap().locale, "en");

    let w = warehouse::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(
        (w.name.as_str(), w.address.as_str(), w.created_by),
        ("白馬倉庫", "第1棟", admin.id)
    );

    let p = project::Entity::find()
        .filter(project::Column::Code.eq("KATAKURI-01"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p.currency, "USD");
    assert_eq!(p.uid.len(), 36, "uid が採番されていません");
    assert_eq!(メンバー数(db, p.id).await, 2);

    let run = import_run::Entity::find_by_id(実行.import_run_id.unwrap())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((run.kind.as_str(), run.project_id), ("organization", None));
}

/// **同じファイルを2回流しても結果が変わらないこと**（23.1）。
async fn 二度流しても変わらない(db: &DatabaseConnection) {
    let _ = 管理者(db).await;
    let dir = 取込ファイル(&[
        (
            "user",
            "users.csv",
            &format!("{利用者の見出し}hotaka,穂高 古城,hotaka@example.invalid,ja,active\n"),
        ),
        (
            "warehouse",
            "warehouses.csv",
            "name,address\n白馬倉庫,第1棟\n",
        ),
        (
            "project",
            "projects.csv",
            "uid,code,name,description,currency\n,KATAKURI-01,カタクリ基盤更改,説明,JPY\n",
        ),
        (
            "project_member",
            "members.csv",
            &format!("{メンバーの見出し}KATAKURI-01,hotaka,Administrator,Primary,\n"),
        ),
    ]);

    run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();
    let 二回目 = run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();

    assert_eq!(
        二回目.report.count(Outcome::Created),
        0,
        "{}",
        詳細(&二回目.report)
    );
    assert_eq!(
        二回目.report.count(Outcome::Updated),
        0,
        "{}",
        詳細(&二回目.report)
    );
    assert_eq!(
        二回目.report.count(Outcome::Unchanged),
        4,
        "{}",
        詳細(&二回目.report)
    );
}

/// **取込で作った利用者も、倉庫用のプロジェクトの Viewer になること**（#218、16.1）。
///
/// - 無効で作った利用者も加える。有効に戻したときに在庫が見えないままにしない
/// - **作る前からいる利用者は加えない**
/// - メンバーのCSVに同じ行（倉庫・Viewer）があっても重ならず、変更なしになる
/// - 加えたことは行ごとに監査ログに残る（23.8、24.4の例外）
/// - 二度流しても増えない
async fn 取り込んだ利用者は倉庫プロジェクトのviewerになる(
    db: &DatabaseConnection,
) {
    let admin = 管理者(db).await;
    let 倉庫 = プロジェクト(db, "SOKO").await;
    app_setting::ActiveModel {
        id: Set(app_setting::ID),
        default_project_id: Set(倉庫.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
    }
    .insert(db)
    .await
    .unwrap();
    let 前からいる = 利用者(db, "kita", false).await;

    let dir = 取込ファイル(&[
        (
            "user",
            "users.csv",
            &format!(
                "{利用者の見出し}hotaka,穂高 古城,,,active\nyari,槍 北鎌,,,disabled\nkita,北 岳,,,\n"
            ),
        ),
        (
            "project_member",
            "members.csv",
            &format!("{メンバーの見出し}SOKO,hotaka,Viewer,,\n"),
        ),
    ]);

    let 一回目 = run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();
    assert!(!一回目.report.has_error(), "{}", 詳細(&一回目.report));

    for username in ["hotaka", "yari"] {
        let u = 利用者を引く(db, username).await.unwrap();
        let 所属: Vec<_> = project_member::Entity::find()
            .filter(project_member::Column::UserId.eq(u.id))
            .all(db)
            .await
            .unwrap();
        assert_eq!(所属.len(), 1, "{username}: {所属:?}");
        assert_eq!(所属[0].project_id, 倉庫.id);
        assert_eq!(所属[0].role, "Viewer");
    }
    let 前からいるの所属 = project_member::Entity::find()
        .filter(project_member::Column::UserId.eq(前からいる.id))
        .count(db)
        .await
        .unwrap();
    assert_eq!(前からいるの所属, 0, "作る前からいる利用者が加わっています");

    // 利用者の行に加えたことが出て、メンバーの行は変更なし
    let hotaka = 一回目
        .report
        .entries
        .iter()
        .find(|e| e.target == "hotaka")
        .unwrap();
    assert!(
        hotaka.detail.contains("Viewer として加えました"),
        "{}",
        hotaka.detail
    );
    let メンバーの行 = 一回目
        .report
        .entries
        .iter()
        .find(|e| e.target.contains("SOKO"))
        .unwrap();
    assert_eq!(
        メンバーの行.outcome,
        Outcome::Unchanged,
        "{}",
        詳細(&一回目.report)
    );

    let 記録 = audit_log::Entity::find()
        .filter(audit_log::Column::TableName.eq("project_member"))
        .all(db)
        .await
        .unwrap();
    assert_eq!(記録.len(), 2, "{記録:?}");
    assert!(記録.iter().all(|r| r.user_id == admin.id));
    assert!(記録.iter().all(|r| r.import_run_id == 一回目.import_run_id));

    run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();
    assert_eq!(メンバー数(db, 倉庫.id).await, 2, "二度目で増えています");
}

/// **パスワードの列があればエラーになること**（23.8）。
async fn パスワードの列があれば拒否する(db: &DatabaseConnection) {
    let _ = 管理者(db).await;
    let dir = 取込ファイル(&[(
        "user",
        "users.csv",
        "username,name,password\nhotaka,穂高 古城,Tanigawa-Bridge-7391\n",
    )]);

    let 結果 = run::run(db, &dir.join("manifest.yaml"), "admin", false).await;
    assert!(
        matches!(結果, Err(RunError::Import(ImportError::Csv(_)))),
        "{:?}",
        結果.err()
    );
    assert!(利用者を引く(db, "hotaka").await.is_none());
}

/// **System Admin を作ろうとするとエラーになること**（23.8）。
///
/// 列で指定する場合も、既存の System Admin を書き換えようとする場合も止める。
async fn system_adminは作れず変えられない(db: &DatabaseConnection) {
    let _ = 管理者(db).await;

    let dir = 取込ファイル(&[(
        "user",
        "users.csv",
        "username,name,is_system_admin\nhotaka,穂高 古城,true\n",
    )]);
    assert!(run::run(db, &dir.join("manifest.yaml"), "admin", false)
        .await
        .is_err());

    let dir = 取込ファイル(&[(
        "user",
        "users.csv",
        &format!("{利用者の見出し}admin,乗っ取り,,,disabled\n"),
    )]);
    let 下見 = run::run(db, &dir.join("manifest.yaml"), "admin", false)
        .await
        .unwrap();
    assert!(
        下見.report.has_error(),
        "既存の System Admin を変更できてしまいます"
    );
    assert!(利用者を引く(db, "admin")
        .await
        .unwrap()
        .disabled_at
        .is_none());
}

/// **ファイルに書かれていない利用者・メンバーが変わらないこと**（23.8）。
///
/// 宣言的な取込をそのまま当てはめると「書かれていない＝無効化・除外」になる。
/// 一部の利用者だけを書いたファイルで、残りが一斉に止まってはならない。
async fn 書かれていない利用者とメンバーは変わらない(db: &DatabaseConnection) {
    let _ = 管理者(db).await;
    let 残る = 利用者(db, "tateyama", false).await;
    let p = プロジェクト(db, "KATAKURI-01").await;
    所属(db, 残る.id, p.id, "Operator", None).await;

    let dir = 取込ファイル(&[
        (
            "user",
            "users.csv",
            &format!("{利用者の見出し}hotaka,穂高 古城,,,\n"),
        ),
        (
            "project_member",
            "members.csv",
            &format!("{メンバーの見出し}KATAKURI-01,hotaka,Viewer,,\n"),
        ),
    ]);
    run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();

    assert!(
        利用者を引く(db, "tateyama")
            .await
            .unwrap()
            .disabled_at
            .is_none(),
        "書かれていない利用者が無効化されました"
    );
    assert_eq!(
        メンバー数(db, p.id).await,
        2,
        "書かれていないメンバーが外されました"
    );
}

/// **明示の無効化・除外は反映されること**（23.8）。
async fn 明示の無効化と除外は反映される(db: &DatabaseConnection) {
    let _ = 管理者(db).await;
    let 去る = 利用者(db, "hakuba", false).await;
    let 外れる = 利用者(db, "tateyama", false).await;
    let p = プロジェクト(db, "KATAKURI-01").await;
    所属(db, 外れる.id, p.id, "Viewer", None).await;

    let dir = 取込ファイル(&[
        (
            "user",
            "users.csv",
            &format!("{利用者の見出し}hakuba,,,,disabled\n"),
        ),
        (
            "project_member",
            "members.csv",
            &format!("{メンバーの見出し}KATAKURI-01,tateyama,Viewer,,true\n"),
        ),
    ]);
    let 実行 = run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();
    assert!(!実行.report.has_error(), "{}", 詳細(&実行.report));

    assert!(
        利用者を引く(db, "hakuba")
            .await
            .unwrap()
            .disabled_at
            .is_some(),
        "無効化されていません"
    );
    assert_eq!(
        去る.name,
        利用者を引く(db, "hakuba").await.unwrap().name,
        "空欄の表示名で上書きされました"
    );
    assert_eq!(メンバー数(db, p.id).await, 0, "除外されていません");
}

/// **利用者の追加・役割付与が監査ログに1行ずつ残ること**（23.8、24.4の例外）。
///
/// 倉庫とプロジェクトは通常の取込と同じく行ごとには残さない。
async fn 利用者と役割の変更は監査ログに残る(db: &DatabaseConnection) {
    let admin = 管理者(db).await;
    let dir = 取込ファイル(&[
        (
            "user",
            "users.csv",
            &format!("{利用者の見出し}hotaka,穂高 古城,,,\ntateyama,立山 滝見,,,\n"),
        ),
        ("warehouse", "warehouses.csv", "name,address\n白馬倉庫,\n"),
        (
            "project",
            "projects.csv",
            "uid,code,name,description,currency\n,KATAKURI-01,カタクリ基盤更改,,\n",
        ),
        (
            "project_member",
            "members.csv",
            &format!("{メンバーの見出し}KATAKURI-01,hotaka,Operator,,\n"),
        ),
    ]);
    let 実行 = run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();
    let run_id = 実行.import_run_id.unwrap();

    let 件数 = |table: &'static str| {
        audit_log::Entity::find()
            .filter(audit_log::Column::TableName.eq(table))
            .filter(audit_log::Column::ImportRunId.eq(run_id))
            .filter(audit_log::Column::UserId.eq(admin.id))
            .count(db)
    };
    assert_eq!(
        件数("app_user").await.unwrap(),
        2,
        "利用者の追加が1行ずつ残っていません"
    );
    assert_eq!(
        件数("project_member").await.unwrap(),
        1,
        "役割の付与が残っていません"
    );
    assert_eq!(件数("warehouse").await.unwrap(), 0);
    assert_eq!(件数("project").await.unwrap(), 0);
}

/// **正管理者が2人になる取込はエラーにし、何も入れないこと**（5章、A-5）。
async fn 正管理者が2人になる取込は拒否する(db: &DatabaseConnection) {
    let _ = 管理者(db).await;
    let dir = 取込ファイル(&[
        ("user", "users.csv", &format!("{利用者の見出し}hotaka,穂高 古城,,,\nhakuba,白馬 石垣,,,\n")),
        ("project", "projects.csv", "uid,code,name,description,currency\n,KATAKURI-01,カタクリ基盤更改,,\n"),
        (
            "project_member",
            "members.csv",
            &format!("{メンバーの見出し}KATAKURI-01,hotaka,Administrator,Primary,\nKATAKURI-01,hakuba,Administrator,Primary,\n"),
        ),
    ]);

    let 下見 = run::run(db, &dir.join("manifest.yaml"), "admin", false)
        .await
        .unwrap();
    assert!(
        下見.report.errors().any(|e| e.detail.contains("正管理者")),
        "{}",
        詳細(&下見.report)
    );
    // 1行につき判定は1つ（23.6）
    assert_eq!(下見.report.entries.len(), 5);

    assert!(run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .is_err());
    assert!(
        利用者を引く(db, "hotaka").await.is_none(),
        "エラーがあるのに反映されました"
    );
}

/// **ドライランは何も残さないこと**（23.6）。
async fn ドライランは何も残さない(db: &DatabaseConnection) {
    let _ = 管理者(db).await;
    let dir = 取込ファイル(&[(
        "user",
        "users.csv",
        &format!("{利用者の見出し}hotaka,穂高 古城,,,\n"),
    )]);

    let 下見 = run::run(db, &dir.join("manifest.yaml"), "admin", false)
        .await
        .unwrap();
    assert_eq!(下見.report.count(Outcome::Created), 1);
    assert!(利用者を引く(db, "hotaka").await.is_none());
    assert_eq!(import_run::Entity::find().count(db).await.unwrap(), 0);
    assert_eq!(audit_log::Entity::find().count(db).await.unwrap(), 0);
}

/// **取り込んだ利用者は、パスワードが設定されるまでログインできないこと**（23.8）。
///
/// 空のハッシュを検証に渡すと形式の誤りで500になる。通常の失敗として扱うこと。
async fn 取り込んだ利用者はリセットまでログインできない(
    db: &DatabaseConnection,
) {
    let _ = 管理者(db).await;
    let dir = 取込ファイル(&[(
        "user",
        "users.csv",
        &format!("{利用者の見出し}hotaka,穂高 古城,,,\n"),
    )]);
    run::run(db, &dir.join("manifest.yaml"), "admin", true)
        .await
        .unwrap();

    let state = 状態(db).await;
    let app = router(state).layer(MockConnectInfo(SocketAddr::from((
        [198, 51, 100, 7],
        40000,
    ))));
    let body = serde_urlencoded::to_string([("username", "hotaka"), ("password", "")]).unwrap();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::OK,
        "ログインできたか、500になりました"
    );
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&bytes).contains("ユーザー名またはパスワードが正しくありません")
    );
}

/// **`docs/examples/organization/` の例がそのまま通ること。**
///
/// 動作確認用データ（#131）の出発点であり、書式の説明にもなっている。例が
/// 壊れたまま残らないようにする。
async fn 例のファイルが通る(db: &DatabaseConnection) {
    let _ = 管理者(db).await;
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/examples/organization/manifest.yaml");

    let 実行 = run::run(db, &manifest, "admin", true).await.unwrap();
    assert!(!実行.report.has_error(), "{}", 詳細(&実行.report));
    assert!(
        利用者を引く(db, "yatsugatake")
            .await
            .unwrap()
            .disabled_at
            .is_some(),
        "例の無効な利用者が無効になっていません"
    );
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn 詳細(report: &dioryga::import::Report) -> String {
    report
        .entries
        .iter()
        .map(|e| format!("{:?} {} {}", e.outcome, e.target, e.detail))
        .collect::<Vec<_>>()
        .join("\n")
}

/// マニフェストとCSVを一時ディレクトリに書き出す。`files` は `(entity, ファイル名, 中身)`。
fn 取込ファイル(files: &[(&str, &str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dioryga-org-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut entries = String::new();
    for (entity, name, body) in files {
        std::fs::write(dir.join(name), body).unwrap();
        entries.push_str(&format!("  - {{ entity: {entity}, path: {name} }}\n"));
    }
    std::fs::write(
        dir.join("manifest.yaml"),
        format!("format_version: 1\nkind: organization\nfiles:\n{entries}"),
    )
    .unwrap();
    dir
}

async fn 管理者(db: &DatabaseConnection) -> app_user::Model {
    利用者(db, "admin", true).await
}

async fn 利用者(db: &DatabaseConnection, username: &str, system_admin: bool) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(format!("{username} の表示名")),
        username: Set(username.to_owned()),
        email: Set(None),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(system_admin),
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

async fn 利用者を引く(db: &DatabaseConnection, username: &str) -> Option<app_user::Model> {
    app_user::Entity::find()
        .filter(app_user::Column::Username.eq(username))
        .one(db)
        .await
        .unwrap()
}

async fn プロジェクト(db: &DatabaseConnection, code: &str) -> project::Model {
    project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(Some(code.to_owned())),
        name: Set(format!("{code} の名前")),
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

async fn 所属(
    db: &DatabaseConnection,
    user_id: i32,
    project_id: i32,
    role: &str,
    rank: Option<&str>,
) {
    project_member::ActiveModel {
        user_id: Set(user_id),
        project_id: Set(project_id),
        role: Set(role.to_owned()),
        admin_rank: Set(rank.map(str::to_owned)),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn メンバー数(db: &DatabaseConnection, project_id: i32) -> u64 {
    project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .count(db)
        .await
        .unwrap()
}

async fn 状態(db: &DatabaseConnection) -> AppState {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    let (setup, _) = SetupState::initialize(db).await.unwrap();
    AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
        staged: Default::default(),
    }
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, system_admin以外は流せない);
        全検証!(@one $用意, $属性, 組織データを取り込める);
        全検証!(@one $用意, $属性, 二度流しても変わらない);
        全検証!(@one $用意, $属性, 取り込んだ利用者は倉庫プロジェクトのviewerになる);
        全検証!(@one $用意, $属性, パスワードの列があれば拒否する);
        全検証!(@one $用意, $属性, system_adminは作れず変えられない);
        全検証!(@one $用意, $属性, 書かれていない利用者とメンバーは変わらない);
        全検証!(@one $用意, $属性, 明示の無効化と除外は反映される);
        全検証!(@one $用意, $属性, 利用者と役割の変更は監査ログに残る);
        全検証!(@one $用意, $属性, 正管理者が2人になる取込は拒否する);
        全検証!(@one $用意, $属性, ドライランは何も残さない);
        全検証!(@one $用意, $属性, 取り込んだ利用者はリセットまでログインできない);
        全検証!(@one $用意, $属性, 例のファイルが通る);
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

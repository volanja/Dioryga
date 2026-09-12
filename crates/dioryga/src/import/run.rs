//! `dioryga import` の実行（設計書23.6、23.7）。
//!
//! **既定はドライラン。**18.2により参照されたカタログ行は編集できず、誤った
//! 取込は事後修正が困難である。書き込むには `--apply` を明示させる。
//!
//! # 誰が取り込んだかを記録する
//!
//! `created_by` と `IMPORT_RUN.imported_by` には実在する利用者が要る。
//! CLIにはログインの概念がないため `--as-user` で指定させ、**その利用者が
//! 実際に取り込んでよいかを確かめる**（18.1）。確かめずに記録すると、
//! 「この取込は誰の責任か」という問いに嘘の答えを残すことになる。

use std::path::Path;

use chrono::Utc;
use entity::{app_user, import_run};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use super::{
    catalog, file_hash, instances, network, parts, placement, ImportError, Outcome, Report,
};
use crate::auth::authorization;
use crate::repository::AuditedTx;

/// 取込の対象と結果。
pub struct Executed {
    pub report: Report,
    /// 反映した場合のみ。ドライランでは `None`。
    pub import_run_id: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("利用者が見つかりません: {0}")]
    UnknownUser(String),

    #[error("この利用者は取込を行えません（いずれかのプロジェクトでOperator以上が要ります）")]
    NotPermitted,

    #[error("この利用者はこのプロジェクトへ取り込めません（Operator以上が要ります）")]
    NotProjectEditor,

    #[error("{0}")]
    Unresolved(String),

    #[error("エンティティ「{0}」の取込はまだ実装されていません")]
    UnsupportedEntity(String),

    #[error(transparent)]
    Import(#[from] ImportError),

    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

/// ファイルの `kind` を見て、どちらの取込かを決める。
///
/// **利用者に種別を指定させない。**ファイル自身が名乗っているものを、こちらが
/// 読み違えないようにするだけでよい。
pub async fn run(
    db: &DatabaseConnection,
    path: &Path,
    as_user: &str,
    apply: bool,
) -> Result<Executed, RunError> {
    let bytes = std::fs::read(path).map_err(ImportError::Io)?;
    let source = String::from_utf8_lossy(&bytes).into_owned();

    #[derive(serde::Deserialize)]
    struct Kind {
        kind: String,
    }
    let kind: Kind = serde_yaml_ng::from_str(&source).map_err(ImportError::Yaml)?;

    match kind.kind.as_str() {
        "catalog" => catalog_file(db, &bytes, &source, as_user, apply).await,
        "instances" => instances_manifest(db, path, &bytes, &source, as_user, apply).await,
        other => Err(ImportError::UnexpectedKind {
            found: other.to_owned(),
            expected: "catalog または instances".to_owned(),
        }
        .into()),
    }
}

/// インスタンスCSVを取り込む（設計書23.5）。
async fn instances_manifest(
    db: &DatabaseConnection,
    path: &Path,
    bytes: &[u8],
    source: &str,
    as_user: &str,
    apply: bool,
) -> Result<Executed, RunError> {
    let manifest = instances::parse_manifest(source)?;

    // **1ファイルは1プロジェクトに閉じる**（23.5）
    let project = instances::解決するプロジェクト(db, &manifest.project)
        .await
        .map_err(ImportError::Db)?
        .map_err(RunError::Unresolved)?;

    // **そのプロジェクトのOperator以上を要する**（23.5）。System Adminは入れない
    let actor = プロジェクトの取込者(db, as_user, project.id).await?;

    let 束 = 読み分ける(path, &manifest.files)?;
    let match_on = manifest.match_on.device.clone();

    let now = Utc::now();
    // **履歴行の from_date は as_of を使う**（23.1）
    let as_of = manifest.as_of.unwrap_or(now);

    // **マニフェスト全体を1つのトランザクションで流す**（23.6）。
    //
    // 以前はドライランを「取込前のDB」に対して行っていた。すると同じ
    // ファイルで作る機器を配置が見つけられずエラーになり、**エラーのある
    // ドライランは反映できないため、初回取込が1回では通らなかった。**
    // `files` を依存順に並べ替えている（23.5）のは、まさにこの参照を
    // 成り立たせるためであり、ドライランもそれに従う必要がある。
    //
    // ドライランは同じ処理を流してから捨てる。**見せた差分と反映の結果が
    // 同じ経路から出る**ため、食い違わない。
    let (tx, run) = AuditedTx::begin_import(
        db,
        import_run::ActiveModel {
            project_id: Set(Some(project.id)),
            kind: Set("instances".to_owned()),
            file_hash: Set(file_hash(bytes)),
            as_of: Set(as_of),
            created_count: Set(0),
            updated_count: Set(0),
            warning_count: Set(0),
            imported_by: Set(actor.id),
            imported_at: Set(now),
            ..Default::default()
        },
    )
    .await?;

    let report = match 通しで取り込む(&tx, project.id, &束, &match_on, as_of, actor.id).await
    {
        Ok(r) => r,
        Err(e) => {
            tx.rollback().await?;
            return Err(e.into());
        }
    };

    if !apply {
        // **ドライランは何も残さない。**IMPORT_RUN も一緒に消える
        tx.rollback().await?;
        return Ok(Executed {
            report,
            import_run_id: None,
        });
    }
    if report.has_error() {
        // **部分的に入れない。**どこまで入ったのかを利用者が把握できない
        tx.rollback().await?;
        return Err(ImportError::HasErrors(report.count(Outcome::Error)).into());
    }

    tx.record_import_counts(
        &run,
        report.count(Outcome::Created) as i32,
        report.count(Outcome::Updated) as i32,
        report.count(Outcome::Warning) as i32,
    )
    .await?;
    tx.commit().await?;

    Ok(Executed {
        report,
        import_run_id: Some(run.id),
    })
}

// ---------------------------------------------------------------------------
// エンティティの振り分け（設計書23.5）
// ---------------------------------------------------------------------------

/// マニフェストが指す全ファイルを読み、エンティティごとにまとめたもの。
#[derive(Default)]
struct 束 {
    devices: Vec<instances::DeviceRow>,
    assignments: Vec<placement::AssignmentRow>,
    mounts: Vec<placement::MountRow>,
    parts: Vec<parts::PartRow>,
    subnets: Vec<network::SubnetRow>,
    interfaces: Vec<network::InterfaceRow>,
    stacks: Vec<network::StackRow>,
    interface_vlans: Vec<network::InterfaceVlanRow>,
    ips: Vec<network::IpRow>,
}

/// マニフェストの `files` を読み分ける。
///
/// **`files` の順序は解釈しない**（23.5）。利用者に依存順を守らせると、
/// 間違えたときのエラーが「参照先が無い」としか出ず、**原因が順序だと
/// 気付けない。**エンティティごとに集めてから、こちらが依存順に処理する。
fn 読み分ける(manifest_path: &Path, files: &[instances::FileRef]) -> Result<束, RunError> {
    let mut out = 束::default();

    for file in files {
        let csv_path = instances::resolve(manifest_path, &file.path);
        let csv = std::fs::read_to_string(&csv_path).map_err(ImportError::Io)?;

        match file.entity.as_str() {
            "device" => out.devices.extend(instances::parse_devices(&csv)?),
            "device_assignment" => out.assignments.extend(placement::parse_assignments(&csv)?),
            "device_mount" => out.mounts.extend(placement::parse_mounts(&csv)?),
            "part_instance" => out.parts.extend(parts::parse_parts(&csv)?),
            "subnet" => out.subnets.extend(network::parse_subnets(&csv)?),
            "os_interface" => out.interfaces.extend(network::parse_interfaces(&csv)?),
            "interface_stack" => out.stacks.extend(network::parse_stacks(&csv)?),
            "interface_vlan" => out
                .interface_vlans
                .extend(network::parse_interface_vlans(&csv)?),
            "ip_address" => out.ips.extend(network::parse_ips(&csv)?),
            other => return Err(RunError::UnsupportedEntity(other.to_owned())),
        }
    }

    Ok(out)
}

/// 依存順に、同じトランザクションの中で流す。
///
/// **機器 → 所属 → 搭載 → 部品 → サブネット → インタフェース → 束ね →
/// VLAN → IPアドレスの順。**後のエンティティは前のエンティティが書いた行を、
/// 同じトランザクションの中で参照する。
async fn 通しで取り込む(
    tx: &AuditedTx,
    project_id: i32,
    束: &束,
    match_on: &[String],
    as_of: chrono::DateTime<Utc>,
    actor: i32,
) -> Result<Report, ImportError> {
    let mut report = instances::取り込む(tx, project_id, &束.devices, match_on, as_of).await?;
    束ねる(
        &mut report,
        placement::所属を取り込む(tx, project_id, &束.assignments, as_of).await?,
    );
    束ねる(
        &mut report,
        placement::搭載を取り込む(tx, project_id, &束.mounts, as_of).await?,
    );
    束ねる(
        &mut report,
        parts::取り込む(tx, project_id, &束.parts, as_of).await?,
    );
    // **サブネットはインタフェースより先。**IPアドレスが参照する
    束ねる(
        &mut report,
        network::サブネットを取り込む(tx, project_id, &束.subnets, actor).await?,
    );
    束ねる(
        &mut report,
        network::インタフェースを取り込む(tx, project_id, &束.interfaces, as_of).await?,
    );
    束ねる(
        &mut report,
        network::束ねを取り込む(tx, project_id, &束.stacks, as_of).await?,
    );
    束ねる(
        &mut report,
        network::インタフェースのvlanを取り込む(
            tx,
            project_id,
            &束.interface_vlans,
            as_of,
        )
        .await?,
    );
    束ねる(
        &mut report,
        network::ipアドレスを取り込む(tx, project_id, &束.ips, as_of).await?,
    );
    Ok(report)
}

fn 束ねる(into: &mut Report, other: Report) {
    into.entries.extend(other.entries);
}

/// カタログYAMLを取り込む。
async fn catalog_file(
    db: &DatabaseConnection,
    bytes: &[u8],
    source: &str,
    as_user: &str,
    apply: bool,
) -> Result<Executed, RunError> {
    let actor = 取込者(db, as_user).await?;
    let file = catalog::parse(source).map_err(ImportError::from)?;

    let report = catalog::dry_run(db, &file).await?;

    if !apply {
        return Ok(Executed {
            report,
            import_run_id: None,
        });
    }

    // **エラーが1件でもあれば反映しない。**部分的に入ると、どこまで入ったのかを
    // 利用者が把握できない
    if report.has_error() {
        return Err(ImportError::HasErrors(report.count(Outcome::Error)).into());
    }

    let now = Utc::now();
    // 記録を先に作る。取り込んだ行から参照される（23.7）
    let run = import_run::ActiveModel {
        // カタログ取込はプロジェクトに属さない（18.1）
        project_id: Set(None),
        kind: Set("catalog".to_owned()),
        file_hash: Set(file_hash(bytes)),
        // カタログYAMLには as_of が無い。実行時刻を基準とする（23.1）
        as_of: Set(now),
        created_count: Set(report.count(Outcome::Created) as i32),
        updated_count: Set(report.count(Outcome::Updated) as i32),
        warning_count: Set(report.count(Outcome::Warning) as i32),
        imported_by: Set(actor.id),
        imported_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await?;

    let report = catalog::apply(db, &file, actor.id, run.id).await?;

    Ok(Executed {
        report,
        import_run_id: Some(run.id),
    })
}

/// 取込を行う利用者を解決し、権限を確かめる（設計書18.1）。
async fn 取込者(db: &DatabaseConnection, email: &str) -> Result<app_user::Model, RunError> {
    let user = app_user::Entity::find()
        .filter(app_user::Column::Email.eq(email))
        .one(db)
        .await?
        .ok_or_else(|| RunError::UnknownUser(email.to_owned()))?;

    // カタログの作成・編集は「いずれか1つ以上のプロジェクトでOperator以上」。
    // **System Adminは含まれない**（3章）
    authorization::require_catalog_editor(db, &user)
        .await
        .map_err(|_| RunError::NotPermitted)?;

    Ok(user)
}

/// プロジェクトへ取り込む利用者を解決し、権限を確かめる（設計書23.5）。
///
/// **System Adminは弾かれる**（3章）。`require_project_editor` が
/// `deny_system_admin` を通しているため、ここで特別扱いする必要はない。
async fn プロジェクトの取込者(
    db: &DatabaseConnection,
    email: &str,
    project_id: i32,
) -> Result<app_user::Model, RunError> {
    let user = app_user::Entity::find()
        .filter(app_user::Column::Email.eq(email))
        .one(db)
        .await?
        .ok_or_else(|| RunError::UnknownUser(email.to_owned()))?;

    authorization::require_project_editor(db, &user, project_id)
        .await
        .map_err(|_| RunError::NotProjectEditor)?;

    Ok(user)
}

/// 差分レポートを標準出力へ書く。
pub fn print_report(executed: &Executed, apply: bool) {
    println!();
    println!("  {}", executed.report);
    println!();

    for entry in executed.report.errors() {
        println!("  エラー  {} — {}", entry.target, entry.detail);
    }
    for entry in executed.report.warnings() {
        println!("  警告    {} — {}", entry.target, entry.detail);
    }

    if executed.report.errors().count() > 0 || executed.report.warnings().count() > 0 {
        println!();
    }

    match executed.import_run_id {
        Some(id) => println!("  反映しました（IMPORT_RUN #{id}）"),
        None if apply => println!("  反映していません"),
        None => println!("  ドライランです。反映するには --apply を付けてください"),
    }
    println!();
}

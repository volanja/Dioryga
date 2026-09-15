//! 組織データの取込（設計書23.8、#130）。
//!
//! 利用者・倉庫・プロジェクト・メンバーを扱う。**System Adminだけが流す**
//! （3章の境界。プロジェクトのデータとは別のマニフェストにする）。
//!
//! ```yaml
//! format_version: 1
//! kind: organization
//! files:
//!   - { entity: user,           path: users.csv }
//!   - { entity: warehouse,      path: warehouses.csv }
//!   - { entity: project,        path: projects.csv }
//!   - { entity: project_member, path: members.csv }
//! ```
//!
//! # 書かれていない行には触らない
//!
//! **追加と更新だけを行う。**無効化（利用者の `status=disabled`）とプロジェクト
//! からの除外（メンバーの `remove=true`）は、ファイルに明示したときだけ行う。
//! 一部の利用者だけを書いたファイルで、残りが一斉にログインできなくなる事故を
//! 防ぐ。
//!
//! **空欄は「変えない」を意味する**（任意の列）。列ごと省いたファイルで、
//! 既存のメールアドレス等が消えないようにするため。
//!
//! # 利用者と役割の変更は、行ごとに監査ログを残す
//!
//! 24.4の「取込は行ごとの監査ログを書かない」の例外（23.8）。倉庫とプロジェクトは
//! 通常の取込と同じく `IMPORT_RUN` が追跡を担う。
//!
//! # System Adminは作れない・変えられない
//!
//! 利用者のCSVに System Admin の列があればファイルごと拒否し、既存の System Admin
//! の行もエラーにする。管理者を作る経路は初回セットアップと `admin create` に限る。
//!
//! # パスワードは書かせない
//!
//! パスワードの列があればファイルごと拒否する。新しい利用者は**パスワード未設定**
//! （空の `password_hash`）で登録し、管理者がリセットするまでログインできない。

use std::collections::HashMap;

use chrono::Utc;
use entity::{app_user, project, project_member, warehouse};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use super::instances::FileRef;
use super::{Entry, ImportError, Outcome, Report};
use crate::auth::authorization::{ADMINISTRATOR, APPROVER, OPERATOR, VIEWER};
use crate::auth::{session, username};
use crate::currency;
use crate::repository::AuditedTx;

pub const KIND: &str = "organization";
const FORMAT_VERSION: u32 = 1;

const PRIMARY: &str = "Primary";
const SECONDARY: &str = "Secondary";
const ROLES: &[&str] = &[ADMINISTRATOR, OPERATOR, APPROVER, VIEWER];

// ---------------------------------------------------------------------------
// マニフェスト
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub kind: String,
    #[serde(default)]
    pub files: Vec<FileRef>,
}

pub fn parse_manifest(source: &str) -> Result<Manifest, ImportError> {
    let manifest: Manifest = serde_yaml_ng::from_str(source)?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(ImportError::UnsupportedVersion {
            found: manifest.format_version,
            expected: FORMAT_VERSION,
        });
    }
    if manifest.kind != KIND {
        return Err(ImportError::UnexpectedKind {
            found: manifest.kind.clone(),
            expected: KIND.to_owned(),
        });
    }
    Ok(manifest)
}

/// マニフェストが指すファイルを、エンティティごとにまとめたもの。
#[derive(Debug, Default)]
pub struct 組織の束 {
    pub users: Vec<UserRow>,
    pub warehouses: Vec<WarehouseRow>,
    pub projects: Vec<ProjectRow>,
    pub members: Vec<MemberRow>,
}

// ---------------------------------------------------------------------------
// 行
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct UserRow {
    pub username: String,
    /// 新しい利用者には必須。既存の利用者では空欄なら変えない。
    #[serde(default)]
    pub name: String,
    /// 任意。空欄なら変えない。
    #[serde(default)]
    pub email: String,
    /// `ja` / `en`。空欄なら変えない（新しい利用者は `ja`）。
    #[serde(default)]
    pub locale: String,
    /// `active` / `disabled`。**無効化はここに明示したときだけ行う。**
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WarehouseRow {
    pub name: String,
    /// 空欄なら変えない。
    #[serde(default)]
    pub address: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectRow {
    /// 既存のプロジェクトを指す場合だけ書く。無ければエラー（採番はDioryga側）。
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 空欄なら変えない（新しいプロジェクトは `JPY`）。
    #[serde(default)]
    pub currency: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemberRow {
    /// `uid` → `code` → `name` の順で解決する（5.3）。
    pub project: String,
    pub username: String,
    pub role: String,
    /// `role=Administrator` のときだけ `Primary` / `Secondary` を書く。
    #[serde(default)]
    pub admin_rank: String,
    /// `true` なら、このロールでの所属を外す。**除外はここに明示したときだけ行う。**
    #[serde(default)]
    pub remove: String,
}

/// 利用者のCSVに書かせない列（23.8）。
const 書かせない列: &[&str] = &[
    "password",
    "password_hash",
    "is_system_admin",
    "system_admin",
];

pub fn parse_users(source: &str) -> Result<Vec<UserRow>, ImportError> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(source.as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| ImportError::Csv(e.to_string()))?;
    for header in headers {
        let header = header.trim().to_ascii_lowercase();
        if 書かせない列.contains(&header.as_str()) {
            return Err(ImportError::Csv(format!(
                "利用者のCSVに「{header}」の列は書けません。パスワードとSystem Adminは取込で扱いません（設計書23.8）"
            )));
        }
    }
    super::placement::読み取る(source)
}

pub fn parse_warehouses(source: &str) -> Result<Vec<WarehouseRow>, ImportError> {
    super::placement::読み取る(source)
}

pub fn parse_projects(source: &str) -> Result<Vec<ProjectRow>, ImportError> {
    super::placement::読み取る(source)
}

pub fn parse_members(source: &str) -> Result<Vec<MemberRow>, ImportError> {
    super::placement::読み取る(source)
}

// ---------------------------------------------------------------------------
// 取込
// ---------------------------------------------------------------------------

/// 依存順（利用者 → 倉庫 → プロジェクト → メンバー）に、同じトランザクションで流す。
///
/// `by` は取込を行う System Admin。倉庫の `created_by` と、監査ログの主体になる。
pub async fn 取り込む(
    tx: &AuditedTx, 束: &組織の束, by: i32
) -> Result<Report, ImportError> {
    let mut report = 利用者を取り込む(tx, &束.users, by).await?;
    report
        .entries
        .extend(倉庫を取り込む(tx, &束.warehouses, by).await?.entries);
    report
        .entries
        .extend(プロジェクトを取り込む(tx, &束.projects).await?.entries);
    report
        .entries
        .extend(メンバーを取り込む(tx, &束.members, by).await?.entries);
    Ok(report)
}

async fn 利用者を取り込む(
    tx: &AuditedTx,
    rows: &[UserRow],
    by: i32,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let mut ユーザー名: HashMap<String, usize> = HashMap::new();
    let mut メール: HashMap<String, usize> = HashMap::new();

    for (i, row) in rows.iter().enumerate() {
        let target = row.username.trim().to_owned();
        let 誤り = |detail: String| Entry::new(Outcome::Error, target.clone(), detail);

        let username = match username::検証する(&row.username) {
            Ok(v) => v,
            Err(e) => {
                report.push(誤り(e.to_string()));
                continue;
            }
        };
        if let Some(前) = ユーザー名.insert(username.clone(), i) {
            report.push(誤り(
                format!("ユーザー名が{}行目と重複しています", 前 + 2),
            ));
            continue;
        }

        let email = username::任意のメールアドレス(&row.email);
        if let Some(e) = &email {
            if let Some(前) = メール.insert(e.clone(), i) {
                report.push(誤り(format!(
                    "メールアドレス「{e}」が{}行目と重複しています",
                    前 + 2
                )));
                continue;
            }
        }

        let locale = match row.locale.trim() {
            "" => None,
            v @ ("ja" | "en") => Some(v.to_owned()),
            other => {
                report.push(誤り(format!("locale「{other}」は使えません（ja / en）")));
                continue;
            }
        };
        // Some(true) = 無効にする、Some(false) = 有効にする、None = 変えない
        let 無効 = match row.status.trim() {
            "" => None,
            "active" => Some(false),
            "disabled" => Some(true),
            other => {
                report.push(誤り(format!(
                    "status「{other}」は使えません（active / disabled）"
                )));
                continue;
            }
        };
        let name = row.name.trim();

        let 既存 = app_user::Entity::find()
            .filter(app_user::Column::Username.eq(&username))
            .one(tx.reader())
            .await?;

        // メールアドレスは、値がある場合は一意（20.1）
        if let Some(e) = &email {
            if let Some(他) = app_user::Entity::find()
                .filter(app_user::Column::Email.eq(e.as_str()))
                .one(tx.reader())
                .await?
            {
                if 既存.as_ref().map(|u| u.id) != Some(他.id) {
                    report.push(誤り(format!(
                        "メールアドレス「{e}」は他の利用者が使っています"
                    )));
                    continue;
                }
            }
        }

        let now = Utc::now();
        let Some(user) = 既存 else {
            if name.is_empty() {
                report.push(誤り(
                    "新しい利用者には表示名（name）が要ります".to_owned(),
                ));
                continue;
            }
            tx.insert_recorded(
                app_user::ActiveModel {
                    name: Set(name.to_owned()),
                    username: Set(username.clone()),
                    email: Set(email),
                    // **パスワード未設定。**管理者がリセットするまでログインできない（23.8）
                    password_hash: Set(String::new()),
                    // **変更の強制はリセットが立てる。**ここで立てても意味が無い——
                    // 空のハッシュでは誰もログインできず、ログインできるようにする
                    // 唯一の経路（admin reset-password）が一時パスワードと同時に立てる。
                    // 立てておくと、開発専用の自動ログイン（#128）で全画面がパスワード
                    // 変更へ飛ばされ、取り込んだ利用者として画面を確かめられない（#131）
                    must_change_password: Set(false),
                    is_system_admin: Set(false),
                    locale: Set(locale.unwrap_or_else(|| "ja".to_owned())),
                    last_login_at: Set(None),
                    disabled_at: Set(無効.unwrap_or(false).then_some(now)),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                },
                by,
            )
            .await?;
            report.push(Entry::new(
                Outcome::Created,
                username,
                "パスワード未設定。管理者がリセットするまでログインできません",
            ));
            continue;
        };

        if user.is_system_admin {
            report.push(誤り(
                "System Adminは取込で変更できません（初回セットアップ・admin create の経路に限る）"
                    .to_owned(),
            ));
            continue;
        }

        let mut active: app_user::ActiveModel = user.clone().into();
        let mut 変更 = false;
        if !name.is_empty() && name != user.name {
            active.name = Set(name.to_owned());
            変更 = true;
        }
        if let Some(e) = &email {
            if user.email.as_deref() != Some(e.as_str()) {
                active.email = Set(Some(e.clone()));
                変更 = true;
            }
        }
        if let Some(l) = &locale {
            if &user.locale != l {
                active.locale = Set(l.clone());
                変更 = true;
            }
        }
        let mut 無効にした = false;
        match 無効 {
            Some(true) if user.disabled_at.is_none() => {
                active.disabled_at = Set(Some(now));
                変更 = true;
                無効にした = true;
            }
            Some(false) if user.disabled_at.is_some() => {
                active.disabled_at = Set(None);
                変更 = true;
            }
            _ => {}
        }

        if !変更 {
            report.push(Entry::new(Outcome::Unchanged, username, ""));
            continue;
        }
        active.updated_at = Set(now);
        tx.update_recorded(&user, active, by).await?;

        if 無効にした {
            // 画面の無効化と同じく、発行済みのセッションを失効させる（20.11）。
            // セッションは監査ログの対象外（24.4）。ドライランでは一緒に捨てられる
            session::revoke_all_except(tx.reader(), user.id, None, now)
                .await
                .map_err(|e| ImportError::Db(sea_orm::DbErr::Custom(e.to_string())))?;
        }
        report.push(Entry::new(
            Outcome::Updated,
            username,
            if 無効にした {
                "無効化しました"
            } else {
                ""
            },
        ));
    }

    Ok(report)
}

async fn 倉庫を取り込む(
    tx: &AuditedTx,
    rows: &[WarehouseRow],
    by: i32,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let mut 出現: HashMap<String, usize> = HashMap::new();

    for (i, row) in rows.iter().enumerate() {
        let name = row.name.trim().to_owned();
        if name.is_empty() {
            report.push(Entry::new(Outcome::Error, "(倉庫)", "name が空です"));
            continue;
        }
        if let Some(前) = 出現.insert(name.clone(), i) {
            report.push(Entry::new(
                Outcome::Error,
                name,
                format!("倉庫名が{}行目と重複しています", 前 + 2),
            ));
            continue;
        }

        let 既存 = warehouse::Entity::find()
            .filter(warehouse::Column::Name.eq(&name))
            .all(tx.reader())
            .await?;
        let address = row.address.trim();
        let now = Utc::now();

        match 既存.as_slice() {
            [] => {
                tx.insert(warehouse::ActiveModel {
                    name: Set(name.clone()),
                    address: Set(address.to_owned()),
                    created_by: Set(by),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?;
                report.push(Entry::new(Outcome::Created, name, ""));
            }
            [w] => {
                if address.is_empty() || address == w.address {
                    report.push(Entry::new(Outcome::Unchanged, name, ""));
                    continue;
                }
                let mut active: warehouse::ActiveModel = w.clone().into();
                active.address = Set(address.to_owned());
                active.updated_at = Set(now);
                tx.update(w, active).await?;
                report.push(Entry::new(Outcome::Updated, name, ""));
            }
            多数 => report.push(Entry::new(
                Outcome::Error,
                name,
                format!(
                    "同じ名前の倉庫が{}件あります。取込では区別できません",
                    多数.len()
                ),
            )),
        }
    }

    Ok(report)
}

async fn プロジェクトを取り込む(
    tx: &AuditedTx,
    rows: &[ProjectRow],
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let mut 出現: HashMap<i32, usize> = HashMap::new();
    let mut 新規のcode: HashMap<String, usize> = HashMap::new();

    for (i, row) in rows.iter().enumerate() {
        let uid = row.uid.trim();
        let code = row.code.trim();
        let name = row.name.trim();
        let target = [code, name, uid]
            .into_iter()
            .find(|v| !v.is_empty())
            .unwrap_or("(プロジェクト)")
            .to_owned();
        let 誤り = |detail: String| Entry::new(Outcome::Error, target.clone(), detail);

        let currency = match row.currency.trim() {
            "" => None,
            v => {
                let upper = v.to_uppercase();
                if !currency::is_supported(&upper) {
                    report.push(誤り(format!("通貨「{v}」には対応していません")));
                    continue;
                }
                Some(upper)
            }
        };

        // uid → code → name の順で解決する（5.3）
        let 既存 = if !uid.is_empty() {
            match project::Entity::find()
                .filter(project::Column::Uid.eq(uid))
                .one(tx.reader())
                .await?
            {
                Some(p) => Some(p),
                None => {
                    report.push(誤り(
                        format!("uid「{uid}」のプロジェクトが見つかりません"),
                    ));
                    continue;
                }
            }
        } else if !code.is_empty() {
            project::Entity::find()
                .filter(project::Column::Code.eq(code))
                .one(tx.reader())
                .await?
        } else if !name.is_empty() {
            let 同名 = project::Entity::find()
                .filter(project::Column::Name.eq(name))
                .all(tx.reader())
                .await?;
            match 同名.len() {
                0 => None,
                1 => 同名.into_iter().next(),
                n => {
                    report.push(誤り(format!(
                        "名前「{name}」のプロジェクトが{n}件あります。code か uid を書いてください"
                    )));
                    continue;
                }
            }
        } else {
            report.push(誤り("uid・code・name のいずれかが要ります".to_owned()));
            continue;
        };

        let now = Utc::now();
        let Some(p) = 既存 else {
            if name.is_empty() {
                report.push(誤り("新しいプロジェクトには name が要ります".to_owned()));
                continue;
            }
            if !code.is_empty() {
                if let Some(前) = 新規のcode.insert(code.to_owned(), i) {
                    report.push(誤り(format!(
                        "code「{code}」が{}行目と重複しています",
                        前 + 2
                    )));
                    continue;
                }
            }
            tx.insert(project::ActiveModel {
                // 名前を変えても参照が切れないよう、Dioryga側で採番する（5.3）
                uid: Set(uuid::Uuid::new_v4().to_string()),
                code: Set((!code.is_empty()).then(|| code.to_owned())),
                name: Set(name.to_owned()),
                description: Set(row.description.trim().to_owned()),
                currency: Set(currency.unwrap_or_else(|| currency::DEFAULT.to_owned())),
                archived_at: Set(None),
                closure_reason: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await?;
            report.push(Entry::new(Outcome::Created, target, ""));
            continue;
        };

        if let Some(前) = 出現.insert(p.id, i) {
            report.push(誤り(format!(
                "{}行目と同じプロジェクトを指しています",
                前 + 2
            )));
            continue;
        }

        let mut active: project::ActiveModel = p.clone().into();
        let mut 変更 = false;
        let mut 警告 = None;

        if !name.is_empty() && name != p.name {
            active.name = Set(name.to_owned());
            変更 = true;
        }
        if !code.is_empty() && p.code.as_deref() != Some(code) {
            if let Some(他) = project::Entity::find()
                .filter(project::Column::Code.eq(code))
                .one(tx.reader())
                .await?
            {
                if 他.id != p.id {
                    report.push(誤り(format!(
                        "code「{code}」は他のプロジェクトが使っています"
                    )));
                    continue;
                }
            }
            active.code = Set(Some(code.to_owned()));
            変更 = true;
        }
        let description = row.description.trim();
        if !description.is_empty() && description != p.description {
            active.description = Set(description.to_owned());
            変更 = true;
        }
        if let Some(c) = &currency {
            if c != &p.currency {
                active.currency = Set(c.clone());
                変更 = true;
                // 画面と同じく変えられるが、既存の金額の読み方が変わる（24.2.1）
                警告 = Some(format!(
                    "集計通貨を {} から {c} に変えます。既存の金額の解釈が変わります",
                    p.currency
                ));
            }
        }

        if !変更 {
            report.push(Entry::new(Outcome::Unchanged, target, ""));
            continue;
        }
        active.updated_at = Set(now);
        tx.update(&p, active).await?;
        match 警告 {
            Some(detail) => report.push(Entry::new(Outcome::Warning, target, detail)),
            None => report.push(Entry::new(Outcome::Updated, target, "")),
        }
    }

    Ok(report)
}

/// メンバーの1行を解決した結果。
struct 解決済み {
    index: usize,
    target: String,
    project: project::Model,
    user: app_user::Model,
    role: String,
    rank: Option<String>,
    remove: bool,
}

async fn メンバーを取り込む(
    tx: &AuditedTx,
    rows: &[MemberRow],
    by: i32,
) -> Result<Report, ImportError> {
    // **1行につき判定は1つ**（23.6）。依存の都合で処理順と行順が変わるため、
    // 行の位置に判定を置いていく
    let mut 判定: Vec<Option<Entry>> = vec![None; rows.len()];
    let mut 解決: Vec<解決済み> = Vec::new();
    let mut 出現: HashMap<(i32, i32, String), usize> = HashMap::new();

    for (i, row) in rows.iter().enumerate() {
        let target = format!(
            "{} / {} / {}",
            row.project.trim(),
            row.username.trim(),
            row.role.trim()
        );
        let 誤り = |detail: String| Some(Entry::new(Outcome::Error, target.clone(), detail));

        let project =
            match super::instances::解決するプロジェクト(tx.reader(), row.project.trim()).await?
            {
                Ok(p) => p,
                Err(message) => {
                    判定[i] = 誤り(message);
                    continue;
                }
            };

        let key = username::正規化する(&row.username);
        let Some(user) = app_user::Entity::find()
            .filter(app_user::Column::Username.eq(&key))
            .one(tx.reader())
            .await?
        else {
            判定[i] = 誤り(format!("利用者「{key}」が見つかりません"));
            continue;
        };
        if user.is_system_admin {
            判定[i] = 誤り("System Adminはプロジェクトのメンバーにできません（3章）".to_owned());
            continue;
        }

        let role = row.role.trim();
        if !ROLES.contains(&role) {
            判定[i] = 誤り(format!(
                "role「{role}」は使えません（{}）",
                ROLES.join(" / ")
            ));
            continue;
        }

        let rank = row.admin_rank.trim();
        let rank = if role == ADMINISTRATOR {
            match rank {
                PRIMARY | SECONDARY => Some(rank.to_owned()),
                _ => {
                    判定[i] = 誤り(
                        "Administrator には admin_rank（Primary / Secondary）が要ります".to_owned(),
                    );
                    continue;
                }
            }
        } else if rank.is_empty() {
            None
        } else {
            判定[i] = 誤り("admin_rank は Administrator のときだけ書けます".to_owned());
            continue;
        };

        let remove = match row.remove.trim() {
            "" | "false" => false,
            "true" => true,
            other => {
                判定[i] = 誤り(format!("remove「{other}」は使えません（true / false）"));
                continue;
            }
        };

        if !remove && user.disabled_at.is_some() {
            判定[i] = 誤り("無効化された利用者はメンバーに加えられません".to_owned());
            continue;
        }

        if let Some(前) = 出現.insert((project.id, user.id, role.to_owned()), i) {
            判定[i] = 誤り(format!("{}行目と同じ所属を指しています", 前 + 2));
            continue;
        }

        解決.push(解決済み {
            index: i,
            target,
            project,
            user,
            role: role.to_owned(),
            rank,
            remove,
        });
    }

    // **除外 → 正管理者 → 副管理者 → その他の順に流す。**正副の入れ替えを
    // 1つのファイルで書けるようにするため（旧を外してから新を置く）
    解決.sort_by_key(|r| match (r.remove, r.rank.as_deref()) {
        (true, _) => 0,
        (false, Some(PRIMARY)) => 1,
        (false, Some(SECONDARY)) => 2,
        _ => 3,
    });

    let mut 管理者を触った: HashMap<i32, Vec<usize>> = HashMap::new();
    let now = Utc::now();

    for r in &解決 {
        let 既存 = project_member::Entity::find()
            .filter(project_member::Column::ProjectId.eq(r.project.id))
            .filter(project_member::Column::UserId.eq(r.user.id))
            .filter(project_member::Column::Role.eq(r.role.as_str()))
            .one(tx.reader())
            .await?;

        if r.role == ADMINISTRATOR {
            管理者を触った
                .entry(r.project.id)
                .or_default()
                .push(r.index);
        }

        let entry = match (r.remove, 既存) {
            (true, Some(m)) => {
                tx.delete_recorded(m, by).await?;
                Entry::new(
                    Outcome::Updated,
                    r.target.clone(),
                    "プロジェクトから外しました",
                )
            }
            (true, None) => Entry::new(Outcome::Unchanged, r.target.clone(), "所属していません"),
            (false, None) => {
                tx.insert_recorded(
                    project_member::ActiveModel {
                        user_id: Set(r.user.id),
                        project_id: Set(r.project.id),
                        role: Set(r.role.clone()),
                        admin_rank: Set(r.rank.clone()),
                        created_at: Set(now),
                        updated_at: Set(now),
                        ..Default::default()
                    },
                    by,
                )
                .await?;
                Entry::new(Outcome::Created, r.target.clone(), "")
            }
            (false, Some(m)) if m.admin_rank != r.rank => {
                let mut active: project_member::ActiveModel = m.clone().into();
                active.admin_rank = Set(r.rank.clone());
                active.updated_at = Set(now);
                tx.update_recorded(&m, active, by).await?;
                Entry::new(Outcome::Updated, r.target.clone(), "")
            }
            (false, Some(_)) => Entry::new(Outcome::Unchanged, r.target.clone(), ""),
        };
        判定[r.index] = Some(entry);
    }

    // **正副の規則は、流し終えた状態で確かめる**（5章、A-5）。違反は、その
    // プロジェクトの管理者の行（ファイル上の最後の行）をエラーにする
    for (project_id, 行) in 管理者を触った {
        let 管理者 = project_member::Entity::find()
            .filter(project_member::Column::ProjectId.eq(project_id))
            .filter(project_member::Column::Role.eq(ADMINISTRATOR))
            .all(tx.reader())
            .await?;
        let 正: Vec<i32> = 管理者
            .iter()
            .filter(|m| m.admin_rank.as_deref() == Some(PRIMARY))
            .map(|m| m.user_id)
            .collect();
        let 副: Vec<i32> = 管理者
            .iter()
            .filter(|m| m.admin_rank.as_deref() == Some(SECONDARY))
            .map(|m| m.user_id)
            .collect();

        let 違反 = if 正.len() > 1 {
            Some("正管理者（Primary）が2人以上になります")
        } else if 副.len() > 1 {
            Some("副管理者（Secondary）が2人以上になります")
        } else if 正.is_empty() && !副.is_empty() {
            Some("正管理者（Primary）がいないまま副管理者（Secondary）が残ります")
        } else if 正.iter().any(|u| 副.contains(u)) {
            Some("同じ利用者を正管理者と副管理者の両方にはできません")
        } else {
            None
        };

        if let (Some(detail), Some(&最後)) = (違反, 行.iter().max()) {
            let target = 判定[最後]
                .as_ref()
                .map(|e| e.target.clone())
                .unwrap_or_default();
            判定[最後] = Some(Entry::new(Outcome::Error, target, detail));
        }
    }

    Ok(Report {
        entries: 判定.into_iter().flatten().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn マニフェストを読める() {
        let m = parse_manifest(
            "format_version: 1\nkind: organization\nfiles:\n  - { entity: user, path: users.csv }\n",
        )
        .unwrap();
        assert_eq!(m.files.len(), 1);
    }

    #[test]
    fn 別の種類のマニフェストは拒否する() {
        assert!(parse_manifest("format_version: 1\nkind: instances\n").is_err());
    }

    /// **パスワードの列はファイルごと拒否する**（23.8）。
    #[test]
    fn パスワードの列は拒否する() {
        for header in ["password", "password_hash", "Password"] {
            let csv = format!("username,name,{header}\nhotaka,穂高 古城,secret\n");
            assert!(parse_users(&csv).is_err(), "{header}");
        }
    }

    /// **System Admin の列はファイルごと拒否する**（23.8）。
    #[test]
    fn system_adminの列は拒否する() {
        for header in ["is_system_admin", "system_admin"] {
            let csv = format!("username,name,{header}\nhotaka,穂高 古城,true\n");
            assert!(parse_users(&csv).is_err(), "{header}");
        }
    }

    #[test]
    fn 任意の列は省ける() {
        let rows = parse_users("username,name\nhotaka,穂高 古城\n").unwrap();
        assert_eq!(rows[0].email, "");
        assert_eq!(rows[0].status, "");
    }
}

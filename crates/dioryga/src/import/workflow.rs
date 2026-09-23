//! マイルストーンと変更管理チケットの取込（設計書23.5、10.4、11章）。
//!
//! # 突合は `uid` → `external_id`（23.5）
//!
//! どちらの表も識別子を持っていなかった。画面から作る限りは困らないが、
//! **取込には突合キーが要る。**無ければ同じファイルを2回流すたびに増え、
//! 宣言的な取込（23.1）が成り立たない。`DEVICE` と同じ2列にしてある。
//!
//! **`uid` も `external_id` も無い行はエラーにする。**部品でシリアル番号を
//! 必須にしたのと同じ理由で（23.5）、突合できない行は毎回新規になる。
//!
//! # 状態は書かれた値をそのまま入れる
//!
//! **11.2の遷移を取込で再生しない。**移行で持ち込みたいのは「いまどの状態か」
//! であって、そこへ至る操作の列ではない（23.1）。再生させると、過去のチケットを
//! 入れるのに承認 → 実行 → 完了の順で何度も流させることになる。
//!
//! **予定・実行・完了・中止の時刻はマニフェストの `as_of` を使う。**CSVの列には
//! しない——移行元にそろっている保証が無く、列を4つ増やしても埋まらない。
//!
//! # 承認行は別のCSVで書く
//!
//! 移譲では承認が2行になる（11.5）。1行に畳むと承認者の列が2組要り、3つ目が
//! 要る形が来たときに列を足すことになる。
//!
//! **チケットを新しく作るときは、画面と同じ形の承認行（`pending`）を起こす**
//! （11.4-7、11.5）。`approvals.csv` はその行を更新する側になる。承認の無い
//! チケットが取込からだけ生まれると、画面から起票したものと形が違ってしまう。
//!
//! **自己承認（11.4-9）は取込では判定しない。**判定には「他に承認できる
//! メンバーがいたか」が要るが、それは承認した時点の事実であり、移行元の
//! ファイルからは分からない。`self_approved` は画面の承認だけが立てる。
//!
//! # 予約（11.6）は作らない
//!
//! `planned` の `Addition` を取り込んでも `DEVICE_MOUNT` は作らない。搭載は
//! `device_mount` のCSVが書く側であり、取込が別経路で同じ行を作ると二重になる。

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use entity::{app_user, milestone, project_member, work_order, work_order_approval};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use super::instances::{解決するプロジェクト, 語彙};
use super::placement::{機器の索引, 機器キー, 空ならnone, 読み取る};
use super::{Entry, ImportError, Outcome, Report};
use crate::repository::AuditedTx;

/// 語彙（`vocabularies.md`、設計書10.4）。
const MILESTONE_TYPES: &[&str] = &[
    "ServiceStart",
    "ServiceUpdate",
    "ServiceMaintenance",
    "ServiceEnd",
];
const MILESTONE_STATUSES: &[&str] = &["planned", "completed", "cancelled"];

/// 語彙（`vocabularies.md`、設計書11.2、11.3）。**画面（`server::work_order`）と
/// 同じ値を持つ。**片方だけ足すと、画面で選べない値が取込から入る。
const WORK_TYPES: &[&str] = &["Repair", "Addition", "Relocation", "Disposal", "Transfer"];
const WORK_ORDER_STATUSES: &[&str] = &[
    "planned",
    "approved",
    "in_progress",
    "completed",
    "cancelled",
];
const APPROVAL_STATUSES: &[&str] = &["pending", "approved", "rejected"];

const TRANSFER: &str = "Transfer";
const PLANNED: &str = "planned";
const APPROVED: &str = "approved";
const IN_PROGRESS: &str = "in_progress";
const COMPLETED: &str = "completed";
const CANCELLED: &str = "cancelled";
const PENDING: &str = "pending";

/// 承認が済んでいるはずのチケットの状態（11.4-7）。
const 承認済みの状態: &[&str] = &[APPROVED, IN_PROGRESS, COMPLETED];

// ---------------------------------------------------------------------------
// 行の定義
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct MilestoneRow {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub external_id: String,
    pub milestone_type: String,
    pub planned_date: String,
    /// 完了して初めて入る。未完了の行では空（10.4）。
    #[serde(default)]
    pub actual_date: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkOrderRow {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub external_id: String,
    pub work_type: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// 対象機器。**`device_` を冠する**——チケット自身の `uid`/`external_id` と
    /// 同じ名前になってしまうため（23.2の解決順序はそのまま使う）。
    #[serde(default)]
    pub device_uid: String,
    #[serde(default)]
    pub device_external_id: String,
    #[serde(default)]
    pub device_hostname: String,
    #[serde(default)]
    pub device_serial_number: String,
    /// `work_type = Transfer` のときだけ書ける移譲先（11.5）。
    #[serde(default)]
    pub target_project: String,
    #[serde(default)]
    pub primary_assignee: String,
    #[serde(default)]
    pub secondary_assignee: String,
    #[serde(default)]
    pub due_date: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub cancelled_reason: String,
}

impl WorkOrderRow {
    fn 機器キー(&self) -> 機器キー {
        機器キー {
            uid: self.device_uid.clone(),
            external_id: self.device_external_id.clone(),
            hostname: self.device_hostname.clone(),
            serial_number: self.device_serial_number.clone(),
        }
    }

    fn 機器を指しているか(&self) -> bool {
        [
            &self.device_uid,
            &self.device_external_id,
            &self.device_hostname,
            &self.device_serial_number,
        ]
        .iter()
        .any(|v| !v.trim().is_empty())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalRow {
    /// チケットの `uid` か `external_id`。
    pub work_order: String,
    /// どのプロジェクトの承認か。`uid` → `code` → `name` で解決する（5.3）。
    pub required_project: String,
    /// 承認前は空。
    #[serde(default)]
    pub approver: String,
    #[serde(default)]
    pub status: String,
}

pub fn parse_milestones(source: &str) -> Result<Vec<MilestoneRow>, ImportError> {
    読み取る(source)
}
pub fn parse_work_orders(source: &str) -> Result<Vec<WorkOrderRow>, ImportError> {
    読み取る(source)
}
pub fn parse_approvals(source: &str) -> Result<Vec<ApprovalRow>, ImportError> {
    読み取る(source)
}

// ---------------------------------------------------------------------------
// 突合（23.5）
// ---------------------------------------------------------------------------

/// `uid` → `external_id` で既存行を引くための索引。
///
/// **このプロジェクトの行だけを候補にする**（23.5）。1ファイルは1プロジェクトに
/// 閉じており、他プロジェクトの行を書き換えられてはならない。
struct 索引<M> {
    uid: HashMap<String, usize>,
    external_id: HashMap<String, Vec<usize>>,
    行: Vec<M>,
}

impl<M> 索引<M> {
    fn 作る(行: Vec<M>, key: impl Fn(&M) -> (String, Option<String>)) -> Self {
        let mut uid = HashMap::new();
        let mut external_id: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, m) in 行.iter().enumerate() {
            let (u, e) = key(m);
            uid.insert(u, i);
            if let Some(e) = e.filter(|e| !e.trim().is_empty()) {
                external_id.entry(e).or_default().push(i);
            }
        }
        Self {
            uid,
            external_id,
            行,
        }
    }

    /// 突合する。`Ok(None)` は新規、`Err` は解決できない指定。
    fn 引く(&self, uid: &str, external_id: &str) -> Result<Option<&M>, String> {
        if let Some(uid) = 空ならnone(uid) {
            return match self.uid.get(uid) {
                Some(i) => Ok(Some(&self.行[*i])),
                // **uid はDioryga側で採番する。**書いて見つからないのは誤り（23.2）
                None => Err(format!("uid「{uid}」の行が見つかりません")),
            };
        }
        let Some(external_id) = 空ならnone(external_id) else {
            // **毎回新規になる行を作らない**（23.5の部品と同じ）
            return Err(
                "uid と external_id がどちらも空です。再取込で突き合わせられません".to_owned(),
            );
        };
        match self.external_id.get(external_id).map(|v| v.as_slice()) {
            None | Some([]) => Ok(None),
            Some([i]) => Ok(Some(&self.行[*i])),
            // **黙って1件目を選ばない**（23.2）
            Some(v) => Err(format!(
                "external_id「{external_id}」に{}件が該当します。uid で指定してください",
                v.len()
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// マイルストーン（設計書10.4）
// ---------------------------------------------------------------------------

pub async fn マイルストーンを取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[MilestoneRow],
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    if rows.is_empty() {
        return Ok(report);
    }

    let 既存 = milestone::Entity::find()
        .filter(milestone::Column::ProjectId.eq(project_id))
        .all(tx.reader())
        .await?;
    let 索引 = 索引::作る(既存, |m: &milestone::Model| {
        (m.uid.clone(), m.external_id.clone())
    });
    let now = Utc::now();

    for row in rows {
        let target = format!("MILESTONE {}", 表示名(&row.uid, &row.external_id));

        let milestone_type = match 必須の語彙(&row.milestone_type, MILESTONE_TYPES) {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, format!("種別: {e}")));
                continue;
            }
        };
        let Some(planned_date) = 日付(&row.planned_date) else {
            report.push(Entry::new(
                Outcome::Error,
                target,
                "planned_date を日付として読めません",
            ));
            continue;
        };
        let actual_date = match 空ならnone(&row.actual_date) {
            None => None,
            Some(v) => match 日付(v) {
                Some(d) => Some(d),
                None => {
                    report.push(Entry::new(
                        Outcome::Error,
                        target,
                        "actual_date を日付として読めません",
                    ));
                    continue;
                }
            },
        };
        let status = match 語彙(&row.status, MILESTONE_STATUSES, PLANNED) {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, format!("状態: {e}")));
                continue;
            }
        };
        let 既存行 = match 索引.引く(&row.uid, &row.external_id) {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, e));
                continue;
            }
        };
        let description = row.description.trim().to_owned();

        match 既存行 {
            Some(m) => {
                // **一致していれば何も書かない**（23.1）
                if m.milestone_type == milestone_type
                    && m.planned_date == planned_date
                    && m.actual_date == actual_date
                    && m.status == status
                    && m.description == description
                {
                    report.push(Entry::new(Outcome::Unchanged, target, ""));
                    continue;
                }
                tx.update(
                    m,
                    milestone::ActiveModel {
                        id: Set(m.id),
                        milestone_type: Set(milestone_type),
                        planned_date: Set(planned_date),
                        actual_date: Set(actual_date),
                        status: Set(status),
                        description: Set(description),
                        updated_at: Set(now),
                        ..Default::default()
                    },
                )
                .await?;
                report.push(Entry::new(Outcome::Updated, target, ""));
            }
            None => {
                tx.insert(milestone::ActiveModel {
                    uid: Set(採番(&row.uid)),
                    external_id: Set(空ならnone(&row.external_id).map(str::to_owned)),
                    project_id: Set(project_id),
                    milestone_type: Set(milestone_type),
                    planned_date: Set(planned_date),
                    actual_date: Set(actual_date),
                    status: Set(status),
                    description: Set(description),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?;
                report.push(Entry::new(Outcome::Created, target, ""));
            }
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// 変更管理チケット（設計書11章）
// ---------------------------------------------------------------------------

pub async fn チケットを取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[WorkOrderRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    if rows.is_empty() {
        return Ok(report);
    }

    let 機器 = 機器の索引::作る(tx.reader(), project_id).await?;
    let 利用者 = 利用者の索引(tx.reader()).await?;
    let メンバー = メンバーの索引(tx.reader(), project_id).await?;
    let 既存 = work_order::Entity::find()
        .filter(work_order::Column::ProjectId.eq(project_id))
        .all(tx.reader())
        .await?;
    let 索引 = 索引::作る(既存, |w: &work_order::Model| {
        (w.uid.clone(), w.external_id.clone())
    });
    let now = Utc::now();

    for row in rows {
        let target = format!("WORK_ORDER {}", 表示名(&row.uid, &row.external_id));

        let title = row.title.trim().to_owned();
        if title.is_empty() {
            report.push(Entry::new(Outcome::Error, target, "title が空です"));
            continue;
        }
        let work_type = match 必須の語彙(&row.work_type, WORK_TYPES) {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, format!("種別: {e}")));
                continue;
            }
        };
        let status = match 語彙(&row.status, WORK_ORDER_STATUSES, PLANNED) {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, format!("状態: {e}")));
                continue;
            }
        };

        // **移譲先は Transfer のときだけ**（11.5）
        let target_project_id = match (work_type.as_str(), 空ならnone(&row.target_project)) {
            (TRANSFER, None) => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    "Transfer には target_project が要ります",
                ));
                continue;
            }
            (TRANSFER, Some(key)) => match 解決するプロジェクト(tx.reader(), key).await? {
                Ok(p) if p.id == project_id => {
                    report.push(Entry::new(
                        Outcome::Error,
                        target,
                        "target_project が起票元と同じです",
                    ));
                    continue;
                }
                Ok(p) => Some(p.id),
                Err(e) => {
                    report.push(Entry::new(Outcome::Error, target, e));
                    continue;
                }
            },
            (_, Some(_)) => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    "target_project を書けるのは Transfer だけです",
                ));
                continue;
            }
            (_, None) => None,
        };

        let device_id = if row.機器を指しているか() {
            match 機器.引く(&row.機器キー()) {
                Ok(d) => Some(d.id),
                Err(e) => {
                    report.push(Entry::new(Outcome::Error, target, e));
                    continue;
                }
            }
        } else {
            None
        };

        let (primary_assignee_id, 主の指摘) =
            match 担当者(&row.primary_assignee, &利用者, &メンバー) {
                Ok(v) => v,
                Err(e) => {
                    report.push(Entry::new(Outcome::Error, target, format!("主担当: {e}")));
                    continue;
                }
            };
        let (secondary_assignee_id, 副の指摘) =
            match 担当者(&row.secondary_assignee, &利用者, &メンバー) {
                Ok(v) => v,
                Err(e) => {
                    report.push(Entry::new(Outcome::Error, target, format!("副担当: {e}")));
                    continue;
                }
            };

        let due_date = match 空ならnone(&row.due_date) {
            None => None,
            Some(v) => match 日付(v) {
                Some(d) => Some(d),
                None => {
                    report.push(Entry::new(
                        Outcome::Error,
                        target,
                        "due_date を日付として読めません",
                    ));
                    continue;
                }
            },
        };

        let cancelled_reason = 空ならnone(&row.cancelled_reason).map(str::to_owned);
        if status != CANCELLED && cancelled_reason.is_some() {
            report.push(Entry::new(
                Outcome::Error,
                target,
                "cancelled_reason を書けるのは status=cancelled のときだけです",
            ));
            continue;
        }

        let 既存行 = match 索引.引く(&row.uid, &row.external_id) {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, e));
                continue;
            }
        };
        let description = row.description.trim().to_owned();
        let 時刻 = 状態の時刻(&status, as_of);

        let outcome = match 既存行 {
            Some(w) => {
                if w.work_type == work_type
                    && w.title == title
                    && w.description == description
                    && w.device_id == device_id
                    && w.target_project_id == target_project_id
                    && w.primary_assignee_id == primary_assignee_id
                    && w.secondary_assignee_id == secondary_assignee_id
                    && w.due_date == due_date
                    && w.status == status
                    && w.cancelled_reason == cancelled_reason
                {
                    Outcome::Unchanged
                } else {
                    tx.update(
                        w,
                        work_order::ActiveModel {
                            id: Set(w.id),
                            work_type: Set(work_type),
                            title: Set(title),
                            description: Set(description),
                            device_id: Set(device_id),
                            target_project_id: Set(target_project_id),
                            primary_assignee_id: Set(primary_assignee_id),
                            secondary_assignee_id: Set(secondary_assignee_id),
                            due_date: Set(due_date),
                            status: Set(status),
                            planned_at: Set(時刻.planned_at),
                            executed_at: Set(時刻.executed_at),
                            completed_at: Set(時刻.completed_at),
                            cancelled_at: Set(時刻.cancelled_at),
                            cancelled_reason: Set(cancelled_reason),
                            updated_at: Set(now),
                            ..Default::default()
                        },
                    )
                    .await?;
                    Outcome::Updated
                }
            }
            None => {
                let created = tx
                    .insert(work_order::ActiveModel {
                        uid: Set(採番(&row.uid)),
                        external_id: Set(空ならnone(&row.external_id).map(str::to_owned)),
                        project_id: Set(project_id),
                        target_project_id: Set(target_project_id),
                        device_id: Set(device_id),
                        part_instance_id: Set(None),
                        work_type: Set(work_type.clone()),
                        title: Set(title),
                        description: Set(description),
                        primary_assignee_id: Set(primary_assignee_id),
                        secondary_assignee_id: Set(secondary_assignee_id),
                        due_date: Set(due_date),
                        status: Set(status),
                        planned_at: Set(時刻.planned_at),
                        executed_at: Set(時刻.executed_at),
                        completed_at: Set(時刻.completed_at),
                        cancelled_at: Set(時刻.cancelled_at),
                        cancelled_reason: Set(cancelled_reason),
                        created_at: Set(now),
                        updated_at: Set(now),
                        ..Default::default()
                    })
                    .await?;

                // **画面と同じ形の承認行を起こす**（11.4-7、11.5）。
                // approvals.csv はこの行を更新する側になる
                let mut 承認先 = vec![project_id];
                if work_type == TRANSFER {
                    承認先.extend(target_project_id);
                }
                for required in 承認先 {
                    tx.insert(work_order_approval::ActiveModel {
                        work_order_id: Set(created.id),
                        required_project_id: Set(required),
                        approver_id: Set(None),
                        status: Set(PENDING.to_owned()),
                        approved_at: Set(None),
                        self_approved: Set(false),
                        created_at: Set(now),
                        updated_at: Set(now),
                        ..Default::default()
                    })
                    .await?;
                }
                Outcome::Created
            }
        };

        // **担当者がメンバーでないことは止めない**（不変条件6）。移行元の
        // 担当者が今もメンバーとは限らず、拒否すると事実を記録できなくなる
        let 指摘: Vec<&str> = [主の指摘, 副の指摘].into_iter().flatten().collect();
        if 指摘.is_empty() {
            report.push(Entry::new(outcome, target, ""));
        } else {
            report.push(Entry::new(Outcome::Warning, target, 指摘.join(" / ")));
        }
    }

    Ok(report)
}

/// 状態から決まる4つの時刻（11.2）。**`as_of` を使う**——CSVの列にはしない。
#[derive(Default)]
struct 状態の時刻の組 {
    planned_at: Option<DateTime<Utc>>,
    executed_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    cancelled_at: Option<DateTime<Utc>>,
}

fn 状態の時刻(status: &str, as_of: DateTime<Utc>) -> 状態の時刻の組 {
    状態の時刻の組 {
        // 起票は必ず通っている
        planned_at: Some(as_of),
        executed_at: (status == IN_PROGRESS || status == COMPLETED).then_some(as_of),
        completed_at: (status == COMPLETED).then_some(as_of),
        cancelled_at: (status == CANCELLED).then_some(as_of),
    }
}

// ---------------------------------------------------------------------------
// 承認（設計書11.4-7、11.5）
// ---------------------------------------------------------------------------

pub async fn 承認を取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[ApprovalRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    let mut report = Report::default();

    let チケット = work_order::Entity::find()
        .filter(work_order::Column::ProjectId.eq(project_id))
        .all(tx.reader())
        .await?;
    if チケット.is_empty() && rows.is_empty() {
        return Ok(report);
    }
    let 索引 = 索引::作る(チケット.clone(), |w: &work_order::Model| {
        (w.uid.clone(), w.external_id.clone())
    });
    let 利用者 = 利用者の索引(tx.reader()).await?;
    let now = Utc::now();

    for row in rows {
        let target = format!("WORK_ORDER_APPROVAL {}", row.work_order.trim());

        // **チケットは uid でも external_id でも指せる。**どちらで書いたかを
        // 利用者に意識させない
        let key = row.work_order.trim();
        let 対象 = if key.is_empty() {
            Err("work_order が空です".to_owned())
        } else {
            match 索引.引く(key, "") {
                Ok(Some(w)) => Ok(w),
                Ok(None) => unreachable!("uid を渡した引くは None を返さない"),
                Err(_) => match 索引.引く("", key) {
                    Ok(Some(w)) => Ok(w),
                    Ok(None) => Err(format!("チケット「{key}」がこのプロジェクトにありません")),
                    Err(e) => Err(e),
                },
            }
        };
        let 対象 = match 対象 {
            Ok(w) => w,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, e));
                continue;
            }
        };

        let required =
            match 解決するプロジェクト(tx.reader(), row.required_project.trim()).await? {
                Ok(p) => p,
                Err(e) => {
                    report.push(Entry::new(Outcome::Error, target, e));
                    continue;
                }
            };
        let status = match 語彙(&row.status, APPROVAL_STATUSES, PENDING) {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, format!("状態: {e}")));
                continue;
            }
        };
        let approver_id = match 空ならnone(&row.approver) {
            None => None,
            Some(name) => match 利用者.get(name) {
                Some(id) => Some(*id),
                None => {
                    report.push(Entry::new(
                        Outcome::Error,
                        target,
                        format!("承認者「{name}」が見つかりません"),
                    ));
                    continue;
                }
            },
        };
        let approved_at = (status == APPROVED).then_some(as_of);

        let 既存 = work_order_approval::Entity::find()
            .filter(work_order_approval::Column::WorkOrderId.eq(対象.id))
            .filter(work_order_approval::Column::RequiredProjectId.eq(required.id))
            .one(tx.reader())
            .await?;

        let mut outcome = match 既存 {
            Some(a) => {
                if a.status == status && a.approver_id == approver_id {
                    Outcome::Unchanged
                } else {
                    tx.update(
                        &a,
                        work_order_approval::ActiveModel {
                            id: Set(a.id),
                            status: Set(status.clone()),
                            approver_id: Set(approver_id),
                            approved_at: Set(approved_at),
                            updated_at: Set(now),
                            ..Default::default()
                        },
                    )
                    .await?;
                    Outcome::Updated
                }
            }
            None => {
                tx.insert(work_order_approval::ActiveModel {
                    work_order_id: Set(対象.id),
                    required_project_id: Set(required.id),
                    approver_id: Set(approver_id),
                    status: Set(status.clone()),
                    approved_at: Set(approved_at),
                    // **取込では立てない**（11.4-9）。判定に要る事実がファイルに無い
                    self_approved: Set(false),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?;
                Outcome::Created
            }
        };

        let mut 指摘 = String::new();
        if status == APPROVED && approver_id.is_none() {
            指摘 = "承認済みですが承認者が書かれていません".to_owned();
            outcome = Outcome::Warning;
        }
        report.push(Entry::new(outcome, target, 指摘));
    }

    束ねる(&mut report, 承認の整合(tx, project_id).await?);
    Ok(report)
}

/// 承認が揃っていないのに進んでいるチケットを警告する（11.4-7）。
///
/// **止めはしない**（不変条件6）。移行元に承認の記録が無いことは実際にあり、
/// 拒否すると移行できなくなる。**黙って直しもしない**——承認行が真実の源で
/// あり（11章）、辻褄を合わせるために存在しない承認を作るほうが害が大きい。
async fn 承認の整合(tx: &AuditedTx, project_id: i32) -> Result<Report, ImportError> {
    let mut report = Report::default();

    let チケット: Vec<work_order::Model> = work_order::Entity::find()
        .filter(work_order::Column::ProjectId.eq(project_id))
        .filter(work_order::Column::Status.is_in(承認済みの状態.to_vec()))
        .all(tx.reader())
        .await?;
    if チケット.is_empty() {
        return Ok(report);
    }

    let ids: Vec<i32> = チケット.iter().map(|w| w.id).collect();
    let 承認 = work_order_approval::Entity::find()
        .filter(work_order_approval::Column::WorkOrderId.is_in(ids))
        .all(tx.reader())
        .await?;

    for w in チケット {
        let 未了 = 承認
            .iter()
            .filter(|a| a.work_order_id == w.id && a.status != APPROVED)
            .count();
        if 未了 > 0 {
            report.push(Entry::new(
                Outcome::Warning,
                format!(
                    "WORK_ORDER {}",
                    表示名(&w.uid, w.external_id.as_deref().unwrap_or(""))
                ),
                format!(
                    "状態が {} ですが、揃っていない承認が{未了}件あります",
                    w.status
                ),
            ));
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// 共通
// ---------------------------------------------------------------------------

fn 束ねる(into: &mut Report, other: Report) {
    into.entries.extend(other.entries);
}

/// 差分レポートに出す名前。**利用者が書いた識別子をそのまま返す**（23.6）。
fn 表示名(uid: &str, external_id: &str) -> String {
    for candidate in [external_id, uid] {
        let v = candidate.trim();
        if !v.is_empty() {
            return v.to_owned();
        }
    }
    "(識別子なし)".to_owned()
}

/// 書かれていれば使い、無ければ採番する（23.2）。
fn 採番(uid: &str) -> String {
    空ならnone(uid)
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

/// 空を許さない語彙。**既定へ寄せない**——種別は行の意味そのものである。
fn 必須の語彙(value: &str, allowed: &[&'static str]) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("空です（{}）", allowed.join(" / ")));
    }
    allowed
        .iter()
        .find(|v| **v == value)
        .map(|v| (*v).to_owned())
        .ok_or_else(|| format!("「{value}」は使えません（{}）", allowed.join(" / ")))
}

fn 日付(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
}

async fn 利用者の索引<C: ConnectionTrait>(
    db: &C,
) -> Result<HashMap<String, i32>, sea_orm::DbErr> {
    Ok(app_user::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|u| (u.username, u.id))
        .collect())
}

async fn メンバーの索引<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> Result<Vec<i32>, sea_orm::DbErr> {
    Ok(project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.user_id)
        .collect())
}

/// 担当者を引く。**メンバーでないことは警告**（不変条件6）。
fn 担当者(
    username: &str,
    利用者: &HashMap<String, i32>,
    メンバー: &[i32],
) -> Result<(Option<i32>, Option<&'static str>), String> {
    let Some(name) = 空ならnone(username) else {
        return Ok((None, None));
    };
    let Some(id) = 利用者.get(name).copied() else {
        return Err(format!("利用者「{name}」が見つかりません"));
    };
    let 指摘 =
        (!メンバー.contains(&id)).then_some("担当者がこのプロジェクトのメンバーではありません");
    Ok((Some(id), 指摘))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 識別子が空なら突合できないと分かる() {
        let 索引: 索引<milestone::Model> = 索引::作る(Vec::new(), |m| (m.uid.clone(), None));
        let e = 索引.引く("", "").unwrap_err();
        assert!(e.contains("突き合わせられません"), "{e}");
    }

    #[test]
    fn 状態から時刻が決まる() {
        let t = Utc::now();
        let 完了 = 状態の時刻(COMPLETED, t);
        assert_eq!(完了.executed_at, Some(t));
        assert_eq!(完了.completed_at, Some(t));
        assert_eq!(完了.cancelled_at, None);

        let 計画 = 状態の時刻(PLANNED, t);
        assert_eq!(計画.planned_at, Some(t));
        assert_eq!(計画.executed_at, None);
    }

    #[test]
    fn 種別は空を許さない() {
        assert!(必須の語彙("", WORK_TYPES).is_err());
        assert!(必須の語彙("Repair", WORK_TYPES).is_ok());
        assert!(必須の語彙("repair", WORK_TYPES).is_err());
    }
}

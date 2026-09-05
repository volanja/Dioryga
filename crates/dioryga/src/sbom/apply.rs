//! SBOMの反映（設計書9.6の手順1〜5）。
//!
//! # 内容アドレス指定が効く場所
//!
//! ゴールデンイメージから構築された機器のSBOMは完全に一致する（9.7）。
//! **既に同じ `content_hash` があれば圧縮も索引作成も行わない。**保存量だけで
//! なく、取込1回あたりの費用も台数に比例しなくなる。
//!
//! # 監査ログを行ごとに書かない
//!
//! `SBOM_COMPONENT_INDEX` は1回の取込で数千行入りうる。24.4が一括取込について
//! 「行ごとの監査ログを書かない」としたのと同じ理由で、**索引と差分は
//! [`Actor::Import`] で書く。**追跡は `SBOM_IMPORT` の行が担う。

use chrono::{DateTime, Utc};
use entity::{sbom_component_change, sbom_component_index, sbom_import, sbom_snapshot};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};

use super::{差分, Component, Normalized};
use crate::error::{AppError, AppResult};
use crate::repository::AuditedTx;

/// 圧縮の水準。**3は既定値**であり、SBOMのような冗長なテキストでは水準を
/// 上げても比がほとんど伸びず、時間だけが延びる。
const 圧縮水準: i32 = 3;

/// 書き込む前に分かること（設計書9.6の手順1〜3）。
///
/// **一括取込のドライラン（23.6）と同じ形にしている。**`IMPORT_RUN` には
/// 件数を入れる列があり、書き込みを始める前に確定していなければならない。
pub struct Plan {
    pub content_hash: String,
    pub json: String,
    pub component_count: usize,
    /// **既に同じ内容のスナップショットがあった**（9.7）。
    pub snapshot_reused: bool,
    /// 直前の取込と内容が同じだった（9.6の手順3）。
    pub unchanged: bool,
    pub changes: Vec<super::Change>,
    /// 直前の取込。手順4で閉じる。
    previous: Option<sbom_import::Model>,
}

impl Plan {
    pub fn added(&self) -> usize {
        self.数える(super::変化::Added)
    }

    pub fn version_changed(&self) -> usize {
        self.数える(super::変化::VersionChanged)
    }

    pub fn removed(&self) -> usize {
        self.数える(super::変化::Removed)
    }

    fn 数える(&self, kind: super::変化) -> usize {
        self.changes
            .iter()
            .filter(|c| c.change_type == kind)
            .count()
    }
}

/// 取込の結果。画面にそのまま出す。
pub struct Outcome {
    pub sbom_import_id: i32,
}

/// 書き込まずに、何が起きるかだけを求める（設計書9.6の手順1〜3）。
pub async fn 計画<C: ConnectionTrait>(
    db_conn: &C,
    device_id: i32,
    normalized: &Normalized,
) -> AppResult<Plan> {
    let (json, content_hash) = normalized.内容();

    // 手順2：同じハッシュのスナップショットがあるか
    let snapshot_reused = sbom_snapshot::Entity::find_by_id(content_hash.clone())
        .one(db_conn)
        .await
        .map_err(db)?
        .is_some();

    let component_count = {
        let mut c = normalized.components.clone();
        c.sort();
        c.dedup();
        c.len()
    };

    // 手順3：同一Deviceの直前の取込（`superseded_at IS NULL`）
    let previous = sbom_import::Entity::find()
        .filter(sbom_import::Column::DeviceId.eq(device_id))
        .filter(sbom_import::Column::SupersededAt.is_null())
        .one(db_conn)
        .await
        .map_err(db)?;

    let unchanged = previous
        .as_ref()
        .is_some_and(|p| p.content_hash == content_hash);

    // **内容が同じなら差分は0件**（9.6の手順3）。展開すらしない
    let changes = if unchanged {
        Vec::new()
    } else {
        let 前 = match &previous {
            Some(p) => 展開する(db_conn, &p.content_hash).await?,
            None => Vec::new(),
        };
        差分(&前, &normalized.components)
    };

    Ok(Plan {
        content_hash,
        json,
        component_count,
        snapshot_reused,
        unchanged,
        changes,
        previous,
    })
}

/// 計画どおりに書き込む（設計書9.6の手順2、4、5）。
///
/// 呼び出し側がトランザクションを持つ。**索引・差分は行数が多いため
/// `Actor::Import` のトランザクションで呼ぶこと**（24.4）。
pub async fn 反映(
    tx: &AuditedTx,
    device_id: i32,
    imported_by: i32,
    work_order_id: Option<i32>,
    normalized: &Normalized,
    plan: &Plan,
    at: DateTime<Utc>,
) -> AppResult<Outcome> {
    let content_hash = plan.content_hash.clone();
    let json = &plan.json;
    let component_count = plan.component_count;
    let snapshot_reused = plan.snapshot_reused;

    if !snapshot_reused {
        let 圧縮 = zstd::encode_all(json.as_bytes(), 圧縮水準)
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

        tx.insert(sbom_snapshot::ActiveModel {
            content_hash: Set(content_hash.clone()),
            content: Set(圧縮),
            component_count: Set(component_count as i32),
            first_seen_at: Set(at),
        })
        .await
        .map_err(db)?;
    }

    // 手順4：直前の行を閉じる
    if let Some(prev) = &plan.previous {
        tx.update(
            prev,
            sbom_import::ActiveModel {
                id: Set(prev.id),
                superseded_at: Set(Some(at)),
                ..Default::default()
            },
        )
        .await
        .map_err(db)?;
    }

    let 取込 = tx
        .insert(sbom_import::ActiveModel {
            device_id: Set(device_id),
            content_hash: Set(content_hash.clone()),
            source_format: Set(normalized.source_format.to_owned()),
            work_order_id: Set(work_order_id),
            imported_by: Set(imported_by),
            imported_at: Set(at),
            superseded_at: Set(None),
            ..Default::default()
        })
        .await
        .map_err(db)?;

    for c in &plan.changes {
        tx.insert(sbom_component_change::ActiveModel {
            sbom_import_id: Set(取込.id),
            change_type: Set(c.change_type.as_str().to_owned()),
            name: Set(c.name.clone()),
            purl: Set(c.purl.clone()),
            version_from: Set(c.version_from.clone()),
            version_to: Set(c.version_to.clone()),
            ..Default::default()
        })
        .await
        .map_err(db)?;
    }

    // 手順5：**新規の `content_hash` のときだけ**索引を作る（9.8）
    if !snapshot_reused {
        let mut sorted = normalized.components.clone();
        sorted.sort();
        sorted.dedup();
        for c in sorted {
            tx.insert(sbom_component_index::ActiveModel {
                content_hash: Set(content_hash.clone()),
                purl: Set(c.purl.clone()),
                name: Set(c.name.clone()),
                version: Set(c.version.clone()),
                ..Default::default()
            })
            .await
            .map_err(db)?;
        }
    }

    Ok(Outcome {
        sbom_import_id: 取込.id,
    })
}

fn db(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(anyhow::anyhow!(e))
}

/// スナップショットを展開する。
///
/// **真実の源はここ**（9.8）。索引は派生物であり、差分計算には使わない。
pub async fn 展開する<C: ConnectionTrait>(
    db_conn: &C,
    content_hash: &str,
) -> AppResult<Vec<Component>> {
    let Some(s) = sbom_snapshot::Entity::find_by_id(content_hash.to_owned())
        .one(db_conn)
        .await
        .map_err(db)?
    else {
        return Ok(Vec::new());
    };

    let json =
        zstd::decode_all(&s.content[..]).map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(serde_json::from_slice(&json).unwrap_or_default())
}

//! カタログの統合（設計書18.5）。
//!
//! # 削除ではなく転送先の設定
//!
//! 23.9.4と同じ形。吸収された側の行は**削除しない**——`AUDIT_LOG.record_id`
//! が参照しており、物理削除すると監査記録が壊れる。一覧の既定では隠し、
//! 詳細は開けるようにして統合先へ誘導する。
//!
//! # 対象は2つだけ（18.5-3）
//!
//! **統合を作るのは、重複が集計を割るカタログに限る。**`CHASSIS_MODEL` は
//! `CHASSIS_SLOT` の付け替えがスペックそのものの書き換えになり、
//! `CONFIGURATION` は `DEVICE` 側の統合（23.9）で扱える。
//!
//! # 履歴テーブルに触れない
//!
//! `PART_INSTANCE_LOCATION`（履歴）が参照するのは `part_instance_id` であって
//! `part_catalog_id` ではない。**付け替える列がそもそも履歴テーブルに無い**ため、
//! 4章の「記録された事実は変更しない」を破らずに済む（18.5）。
//!
//! # ベンダーの統合はUNIQUE制約と衝突しうる
//!
//! 18.5(a)は「ベンダーを統合すれば下流の重複が消える」としていたが、**実際は
//! 逆で、下流に重複があるとベンダーを統合できない。**`CHASSIS_MODEL` の
//! `UNIQUE(vendor_id, model_name)` 等に抵触するためである。
//!
//! **自動で畳まず、衝突を示して拒否する。**畳むと `PART_CATALOG` に課した
//! スペック一致の条件を迂回でき、さらに `CHASSIS_MODEL` では「作らないと決めた
//! 統合機能」を裏口から作ることになる。

use axum::extract::State;
use axum::response::Response;
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    cable_catalog, chassis_model, configuration_part, maintenance_contract, part_catalog,
    part_instance, purchase_order, recurring_cost, software_catalog, vendor,
};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{入場, Labeled};
use crate::server::view::{render, Chrome};
use crate::server::AppState;

/// 連鎖（A→B→C）を辿る上限。**循環は拒否する**（23.9.4）。
const 連鎖の上限: usize = 16;

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

#[derive(askama::Template)]
#[template(path = "catalog_merge.html")]
struct MergePage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_admin_hint: String,
    t_vendors: String,
    t_vendors_hint: String,
    t_parts: String,
    t_parts_hint: String,
    t_source: String,
    t_target: String,
    t_submit: String,
    t_order_hint: String,
    t_no_candidate: String,
    t_merged: String,
    t_merged_none: String,
    t_from: String,
    t_to: String,
    vendors: Vec<Labeled>,
    parts: Vec<Labeled>,
    merged: Vec<MergedRow>,
    can_merge: bool,
    error: Option<String>,
    notice: Option<String>,
}

struct MergedRow {
    kind: String,
    from: String,
    to: String,
}

pub async fn show(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> AppResult<Response> {
    描く(&state, &current, None, None).await
}

async fn 描く(
    state: &AppState,
    current: &CurrentUser,
    error: Option<String>,
    notice: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    // **統合はプロジェクト横断に影響するため Administrator 以上**（18.5-4）
    let can_merge = authorization::require_catalog_admin(&state.db, &current.user)
        .await
        .is_ok();

    render(&MergePage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "merge"),
        t_title: rust_i18n::t!("merge.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("merge.lead", locale = l).to_string(),
        t_admin_hint: rust_i18n::t!("merge.admin_hint", locale = l).to_string(),
        t_vendors: rust_i18n::t!("merge.vendors", locale = l).to_string(),
        t_vendors_hint: rust_i18n::t!("merge.vendors_hint", locale = l).to_string(),
        t_parts: rust_i18n::t!("merge.parts", locale = l).to_string(),
        t_parts_hint: rust_i18n::t!("merge.parts_hint", locale = l).to_string(),
        t_source: rust_i18n::t!("merge.source", locale = l).to_string(),
        t_target: rust_i18n::t!("merge.target", locale = l).to_string(),
        t_submit: rust_i18n::t!("merge.submit", locale = l).to_string(),
        t_order_hint: rust_i18n::t!("merge.order_hint", locale = l).to_string(),
        t_no_candidate: rust_i18n::t!("merge.no_candidate", locale = l).to_string(),
        t_merged: rust_i18n::t!("merge.merged", locale = l).to_string(),
        t_merged_none: rust_i18n::t!("merge.merged_none", locale = l).to_string(),
        t_from: rust_i18n::t!("merge.from", locale = l).to_string(),
        t_to: rust_i18n::t!("merge.to", locale = l).to_string(),
        vendors: 現役のベンダー(&state.db).await?,
        parts: 現役の部品(&state.db).await?,
        merged: 統合済み(&state.db).await?,
        can_merge,
        error,
        notice,
    })
}

// ---------------------------------------------------------------------------
// ベンダーの統合（設計書18.5）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MergeForm {
    /// 吸収される側。
    pub source_id: i32,
    /// 残る側。
    pub target_id: i32,
}

pub async fn merge_vendor(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<MergeForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    統合権(&state, &current).await?;
    let 訳 = |key: &str| rust_i18n::t!(key, locale = l).to_string();

    if form.source_id == form.target_id {
        return 描く(&state, &current, Some(訳("merge.error_same")), None).await;
    }

    let source = 現役の行(&state.db, form.source_id).await?;

    // **連鎖（A→B→C）を辿ってから判定する**（23.9.4）。既に統合された行を
    // 統合先に指定されたら、その先の生きている行を相手にする
    let target_id = 統合先を辿る(&state.db, form.target_id).await?;
    if target_id == source.id {
        return 描く(&state, &current, Some(訳("merge.error_cycle")), None).await;
    }
    let target = 現役の行(&state.db, target_id).await?;

    // **下流の重複を先に片付けさせる**（18.5）。自動で畳むと、PART_CATALOG に
    // 課したスペック一致の条件を迂回でき、CHASSIS_MODEL では作らないと決めた
    // 統合機能を裏口から作ることになる
    if let Some(衝突) = 衝突を調べる(&state.db, source.id, target.id).await? {
        let e = rust_i18n::t!(
            "merge.error_conflict",
            locale = l,
            kind = 衝突.0,
            name = 衝突.1
        )
        .to_string();
        return 描く(&state, &current, Some(e), None).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();

    // 18.3の各テーブルの `vendor_id` を付け替える
    let mut 件数 = 0;
    件数 += 付け替え_chassis_model(&tx, source.id, target.id, now).await?;
    件数 += 付け替え_part_catalog(&tx, source.id, target.id, now).await?;
    件数 += 付け替え_cable_catalog(&tx, source.id, target.id, now).await?;
    件数 += 付け替え_software_catalog(&tx, source.id, target.id, now).await?;
    件数 += 付け替え_purchase_order(&tx, source.id, target.id, now).await?;
    件数 += 付け替え_maintenance_contract(&tx, source.id, target.id, now).await?;
    件数 += 付け替え_recurring_cost(&tx, source.id, target.id, now).await?;

    // **削除ではなく転送先の設定**（23.9.4）
    tx.update(
        &source,
        vendor::ActiveModel {
            id: Set(source.id),
            merged_into_vendor_id: Set(Some(target.id)),
            merged_at: Set(Some(now)),
            updated_at: Set(now),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let notice = rust_i18n::t!(
        "merge.done_vendor",
        locale = l,
        from = source.name,
        to = target.name,
        count = 件数
    )
    .to_string();
    描く(&state, &current, None, Some(notice)).await
}

// ---------------------------------------------------------------------------
// 部品の統合（設計書18.5）
// ---------------------------------------------------------------------------

pub async fn merge_part(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<MergeForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    統合権(&state, &current).await?;
    let 訳 = |key: &str| rust_i18n::t!(key, locale = l).to_string();

    if form.source_id == form.target_id {
        return 描く(&state, &current, Some(訳("merge.error_same")), None).await;
    }

    let source = 現役の部品行(&state.db, form.source_id).await?;

    let target_id = 部品の統合先を辿る(&state.db, form.target_id).await?;
    if target_id == source.id {
        return 描く(&state, &current, Some(訳("merge.error_cycle")), None).await;
    }
    let target = 現役の部品行(&state.db, target_id).await?;

    // **スペックが一致すること**（18.5）。23.9.3が「構成が異なれば統合しない」
    // としたのと同じ判断で、**違うものを1つに畳むと集計が静かに狂う**
    if source.category != target.category
        || source.core_count != target.core_count
        || source.capacity_gb != target.capacity_gb
        || source.spec_json != target.spec_json
    {
        return 描く(&state, &current, Some(訳("merge.error_spec")), None).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();

    let mut 件数 = 0;
    for row in part_instance::Entity::find()
        .filter(part_instance::Column::PartCatalogId.eq(source.id))
        .all(tx.reader())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        tx.update(
            &row,
            part_instance::ActiveModel {
                id: Set(row.id),
                part_catalog_id: Set(target.id),
                updated_at: Set(now),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        件数 += 1;
    }

    for row in configuration_part::Entity::find()
        .filter(configuration_part::Column::PartCatalogId.eq(source.id))
        .all(tx.reader())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        tx.update(
            &row,
            configuration_part::ActiveModel {
                id: Set(row.id),
                part_catalog_id: Set(target.id),
                updated_at: Set(now),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        件数 += 1;
    }

    tx.update(
        &source,
        part_catalog::ActiveModel {
            id: Set(source.id),
            merged_into_part_catalog_id: Set(Some(target.id)),
            merged_at: Set(Some(now)),
            updated_at: Set(now),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let notice = rust_i18n::t!(
        "merge.done_part",
        locale = l,
        from = source.part_number,
        to = target.part_number,
        count = 件数
    )
    .to_string();
    描く(&state, &current, None, Some(notice)).await
}

// ---------------------------------------------------------------------------
// 衝突の検出（設計書18.5）
// ---------------------------------------------------------------------------

/// ベンダーを統合したときに一意制約へ抵触するか。
///
/// **抵触するなら `(種別, 名前)` を返す。**「統合できません」だけでは、利用者は
/// 何を片付ければよいか分からない（18.5）。
async fn 衝突を調べる<C: ConnectionTrait>(
    db: &C,
    source: i32,
    target: i32,
) -> AppResult<Option<(String, String)>> {
    // UNIQUE(vendor_id, model_name)
    let 元 = chassis_model::Entity::find()
        .filter(chassis_model::Column::VendorId.eq(source))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    for m in 元 {
        let 先 = chassis_model::Entity::find()
            .filter(chassis_model::Column::VendorId.eq(target))
            .filter(chassis_model::Column::ModelName.eq(&m.model_name))
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if 先.is_some() {
            return Ok(Some(("CHASSIS_MODEL".to_owned(), m.model_name)));
        }
    }

    // UNIQUE(vendor_id, part_number)
    let 元 = part_catalog::Entity::find()
        .filter(part_catalog::Column::VendorId.eq(source))
        // **統合済みの行は付け替えないので衝突しない**（上の付け替えを参照）
        .filter(part_catalog::Column::MergedIntoPartCatalogId.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    for p in 元 {
        let 先 = part_catalog::Entity::find()
            .filter(part_catalog::Column::VendorId.eq(target))
            .filter(part_catalog::Column::PartNumber.eq(&p.part_number))
            .filter(part_catalog::Column::MergedIntoPartCatalogId.is_null())
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if 先.is_some() {
            return Ok(Some(("PART_CATALOG".to_owned(), p.part_number)));
        }
    }

    // UNIQUE(name, vendor_id, version)
    let 元 = software_catalog::Entity::find()
        .filter(software_catalog::Column::VendorId.eq(source))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    for s in 元 {
        let 先 = software_catalog::Entity::find()
            .filter(software_catalog::Column::VendorId.eq(target))
            .filter(software_catalog::Column::Name.eq(&s.name))
            .filter(software_catalog::Column::Version.eq(&s.version))
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if 先.is_some() {
            return Ok(Some((
                "SOFTWARE_CATALOG".to_owned(),
                format!("{} {}", s.name, s.version),
            )));
        }
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// 付け替え
// ---------------------------------------------------------------------------

/// `vendor_id` を付け替える。マクロにしないのはテーブルごとに型が違うため。
macro_rules! 付け替え {
    ($名前:ident, $entity:ident, $列:ident) => {
        async fn $名前(
            tx: &AuditedTx,
            source: i32,
            target: i32,
            now: chrono::DateTime<chrono::Utc>,
        ) -> AppResult<usize> {
            let rows = $entity::Entity::find()
                .filter($entity::Column::VendorId.eq(source))
                .all(tx.reader())
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

            let 件数 = rows.len();
            for row in rows {
                tx.update(
                    &row,
                    $entity::ActiveModel {
                        id: Set(row.id),
                        $列: Set(target.into()),
                        updated_at: Set(now),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
            }
            Ok(件数)
        }
    };
}

付け替え!(付け替え_chassis_model, chassis_model, vendor_id);
/// `PART_CATALOG` だけ専用にするのは、**統合済みの行を付け替えないため**である。
///
/// 吸収された側の行は削除せず残るが（23.9.4）、`UNIQUE(vendor_id, part_number)`
/// を掴んだままである。ベンダー統合でこれも付け替えると、**統合先に残っている
/// 同じ型番の行と衝突する。**
///
/// 墓標どうしなので、付け替えなくても困らない——集計はどちらも読まないし、
/// 参照を辿る側は `merged_into_*` の連鎖を追う。
async fn 付け替え_part_catalog(
    tx: &AuditedTx,
    source: i32,
    target: i32,
    now: chrono::DateTime<chrono::Utc>,
) -> AppResult<usize> {
    let rows = part_catalog::Entity::find()
        .filter(part_catalog::Column::VendorId.eq(source))
        .filter(part_catalog::Column::MergedIntoPartCatalogId.is_null())
        .all(tx.reader())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let 件数 = rows.len();
    for row in rows {
        tx.update(
            &row,
            part_catalog::ActiveModel {
                id: Set(row.id),
                vendor_id: Set(target),
                updated_at: Set(now),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }
    Ok(件数)
}
付け替え!(付け替え_cable_catalog, cable_catalog, vendor_id);
付け替え!(付け替え_software_catalog, software_catalog, vendor_id);
付け替え!(付け替え_purchase_order, purchase_order, vendor_id);
付け替え!(
    付け替え_maintenance_contract,
    maintenance_contract,
    vendor_id
);
付け替え!(付け替え_recurring_cost, recurring_cost, vendor_id);

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// 統合先を辿った先の行を返す（設計書23.9.4）。
///
/// **連鎖（A→B→C）を辿り、循環は上限で打ち切る。**循環は登録時に拒否している
/// ので通常は起きないが、辿る側が無限ループしないほうが安全である。
pub(crate) async fn 統合先を辿る<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<i32> {
    let mut 現在 = id;
    for _ in 0..連鎖の上限 {
        let Some(v) = vendor::Entity::find_by_id(現在)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        else {
            return Ok(現在);
        };
        match v.merged_into_vendor_id {
            Some(next) => 現在 = next,
            None => return Ok(現在),
        }
    }
    Ok(現在)
}

pub(crate) async fn 部品の統合先を辿る<C: ConnectionTrait>(
    db: &C,
    id: i32,
) -> AppResult<i32> {
    let mut 現在 = id;
    for _ in 0..連鎖の上限 {
        let Some(p) = part_catalog::Entity::find_by_id(現在)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        else {
            return Ok(現在);
        };
        match p.merged_into_part_catalog_id {
            Some(next) => 現在 = next,
            None => return Ok(現在),
        }
    }
    Ok(現在)
}

async fn 現役の行<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<vendor::Model> {
    vendor::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|v| v.merged_into_vendor_id.is_none())
        .ok_or(AppError::NotFound)
}

async fn 現役の部品行<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<part_catalog::Model> {
    part_catalog::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|p| p.merged_into_part_catalog_id.is_none())
        .ok_or(AppError::NotFound)
}

/// 統合の候補。**吸収済みは出さない。**廃番は出す——廃番と統合は別の判断で、
/// 「もう選ばせない」ことと「同じものだった」ことは違う。
async fn 現役のベンダー<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    Ok(vendor::Entity::find()
        .filter(vendor::Column::MergedIntoVendorId.is_null())
        .order_by_asc(vendor::Column::Name)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|v| Labeled {
            label: v.name,
            value: v.id.to_string(),
        })
        .collect())
}

async fn 現役の部品<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    let parts = part_catalog::Entity::find()
        .filter(part_catalog::Column::MergedIntoPartCatalogId.is_null())
        .order_by_asc(part_catalog::Column::PartNumber)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = Vec::new();
    for p in parts {
        out.push(Labeled {
            label: format!(
                "{} {} ({})",
                ベンダー名(db, p.vendor_id).await?,
                p.part_number,
                p.category
            ),
            value: p.id.to_string(),
        });
    }
    Ok(out)
}

/// 統合済みの一覧。**何が起きたかを見せる**（23.9.4の「必ず記録を残す」）。
async fn 統合済み<C: ConnectionTrait>(db: &C) -> AppResult<Vec<MergedRow>> {
    let mut out = Vec::new();

    for v in vendor::Entity::find()
        .filter(vendor::Column::MergedIntoVendorId.is_not_null())
        .order_by_asc(vendor::Column::Name)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        let 先 = match v.merged_into_vendor_id {
            Some(id) => ベンダー名(db, id).await?,
            None => String::new(),
        };
        out.push(MergedRow {
            kind: "VENDOR".to_owned(),
            from: v.name,
            to: 先,
        });
    }

    for p in part_catalog::Entity::find()
        .filter(part_catalog::Column::MergedIntoPartCatalogId.is_not_null())
        .order_by_asc(part_catalog::Column::PartNumber)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        let 先 = match p.merged_into_part_catalog_id {
            Some(id) => part_catalog::Entity::find_by_id(id)
                .one(db)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                .map(|t| t.part_number)
                .unwrap_or_default(),
            None => String::new(),
        };
        out.push(MergedRow {
            kind: "PART_CATALOG".to_owned(),
            from: p.part_number,
            to: 先,
        });
    }

    Ok(out)
}

async fn ベンダー名<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<String> {
    Ok(vendor::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|v| v.name)
        .unwrap_or_default())
}

async fn 統合権(state: &AppState, current: &CurrentUser) -> AppResult<()> {
    authorization::require_catalog_admin(&state.db, &current.user)
        .await
        .map_err(|_| AppError::Forbidden)
}

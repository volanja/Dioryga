//! 設備・什器（`MOUNT_CONTAINER`）の、画面と取込が共有する規則（設計書12.10、#204）。
//!
//! **名前の比べ方と撤去の判定を1か所に置く。**画面と取込で別々に書くと、
//! 片方だけ直したときに「画面では重複、取込では別物」のように食い違う。

use entity::{container_model, device_mount, mount_container, recurring_cost};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait, QueryFilter};

/// 語彙（`vocabularies.md`、設計書12.10）。**表は `dioryga_catalog_format` にだけ置き、
/// 画面・取込・カタログが同じものを見る**（機器の種別と同じ、8.6）。
pub use dioryga_catalog_format::CONTAINER_TYPES;
pub const RACK: &str = "Rack";
pub const DESK: &str = "Desk";
pub const SHELVING: &str = "Shelving";

/// 設備・什器の種別と収容能力。**型番（`CONTAINER_MODEL`）から引く**（#205）。
#[derive(Debug, Clone, Default)]
pub struct 型 {
    pub container_type: String,
    /// Rack なら総U数、Shelving なら段数、Desk は `None`。
    pub capacity: Option<i32>,
    pub model: Option<container_model::Model>,
}

/// 型番の種別に応じた収容能力（12.10）。
pub fn 収容能力(m: &container_model::Model) -> Option<i32> {
    match m.container_type.as_str() {
        RACK => m.height_u,
        SHELVING => m.shelf_count,
        _ => None,
    }
}

/// 設備・什器の型番を引く。**型番が無い行**（DB上は NULL を許す、移行の都合）は
/// 種別も収容能力も空として扱う。
pub async fn 型を引く<C: ConnectionTrait>(
    db: &C,
    container: &mount_container::Model,
) -> Result<型, DbErr> {
    let Some(id) = container.container_model_id else {
        return Ok(型::default());
    };
    let model = container_model::Entity::find_by_id(id).one(db).await?;
    Ok(match model {
        Some(m) => 型 {
            container_type: m.container_type.clone(),
            capacity: 収容能力(&m),
            model: Some(m),
        },
        None => 型::default(),
    })
}

/// 継続費用が設備・什器を指すときの `item_type`（10章）。
const MOUNT_CONTAINER: &str = "MountContainer";

/// 名前を比べるための鍵。**大文字小文字を区別しない**（12.10）。
///
/// DBの一意インデックスは `lower(name)` だが、SQLite の `lower()` は ASCII しか
/// 小文字にしない。**アプリ層では Unicode の小文字で比べる**ので、DBより厳しく
/// 拒否する側に倒れる。
pub fn 名前の鍵(name: &str) -> String {
    name.trim().to_lowercase()
}

/// 置き場所の中で、同じ名前の**撤去していない**設備を探す。
///
/// `except` は自分自身（撤去の取り消しで、自分と比べないため）。
pub async fn 同じ名前の設備<C: ConnectionTrait>(
    db: &C,
    location_type: &str,
    location_id: i32,
    name: &str,
    except: Option<i32>,
) -> Result<Option<mount_container::Model>, DbErr> {
    let 鍵 = 名前の鍵(name);
    Ok(mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(location_type))
        .filter(mount_container::Column::LocationId.eq(location_id))
        .filter(mount_container::Column::RetiredAt.is_null())
        .all(db)
        .await?
        .into_iter()
        .find(|c| Some(c.id) != except && 名前の鍵(&c.name) == 鍵))
}

/// 撤去したときに何が起きるか（12.10）。倉庫の片付け（16.1）と同じ3分岐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum 撤去の結末 {
    /// 機器が載っている。先に移させる。
    使用中,
    /// 一度も使われていない。行ごと消す。
    物理削除,
    /// 過去に使った。`retired_at` を立てて行を残す。
    撤去,
}

/// **「使った」には、搭載の履歴だけでなく継続費用からの参照も含める。**
/// ラックのレンタル費（`RECURRING_COST`、10章）が指している設備を消すと、
/// 費用の記録が参照先を失う（多態的参照のため外部キーが止めない、24.5）。
pub async fn 撤去の判定<C: ConnectionTrait>(
    db: &C,
    id: i32,
) -> Result<撤去の結末, DbErr> {
    let 載っている = device_mount::Entity::find()
        .filter(device_mount::Column::ContainerId.eq(id))
        .filter(device_mount::Column::ToDate.is_null())
        .count(db)
        .await?;
    if 載っている > 0 {
        return Ok(撤去の結末::使用中);
    }

    let 搭載の履歴 = device_mount::Entity::find()
        .filter(device_mount::Column::ContainerId.eq(id))
        .count(db)
        .await?;
    let 費用 = recurring_cost::Entity::find()
        .filter(recurring_cost::Column::ItemType.eq(MOUNT_CONTAINER))
        .filter(recurring_cost::Column::ItemId.eq(id))
        .count(db)
        .await?;
    Ok(if 搭載の履歴 + 費用 > 0 {
        撤去の結末::撤去
    } else {
        撤去の結末::物理削除
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 大文字小文字と前後の空白を区別しない() {
        assert_eq!(名前の鍵(" R01 "), 名前の鍵("r01"));
        assert_eq!(名前の鍵("Ｒ０１"), 名前の鍵("ｒ０１"));
        assert_ne!(名前の鍵("R01"), 名前の鍵("R02"));
    }
}

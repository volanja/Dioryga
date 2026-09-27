//! 設備・什器（`MOUNT_CONTAINER`）に撤去（`retired_at`）と、名前の一意を足す（#204）。
//!
//! # 名前は置き場所の中で一意、大文字小文字を区別しない
//!
//! 画面で同じ名前の設備を2つ作れてしまい、取込（設計書23.5）がどちらを指すか
//! 決められなかった。**撤去していないものに限る**——撤去済みの名前を、後から
//! 入れ替えた設備に付けることは実際にある。
//!
//! `lower(name)` の式インデックスと部分インデックスは両DBが対応する
//! （不変条件7）。ただし SQLite の `lower()` は ASCII しか小文字にしない。
//! 画面と取込はアプリ層でも Unicode の小文字で比べて拒否するので、DBの制約は
//! その下の最後の守りである。
//!
//! # 既存の重複は名前をずらして解消する
//!
//! 未リリースなので行を作り直してよい（設計書16.6）が、消すと搭載の履歴が
//! 参照先を失う。**後から作ったほうの名前に id を付けて**重複を解消してから
//! インデックスを張る。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const INDEX: &str = "idx_mount_container_name_active";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MountContainer::Table)
                    .add_column(timestamp_with_time_zone_null(MountContainer::RetiredAt))
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();
        db.execute_unprepared(
            "UPDATE mount_container SET name = name || '-' || CAST(id AS TEXT) \
             WHERE EXISTS (SELECT 1 FROM mount_container AS m2 \
               WHERE m2.location_type = mount_container.location_type \
                 AND m2.location_id = mount_container.location_id \
                 AND lower(m2.name) = lower(mount_container.name) \
                 AND m2.id < mount_container.id)",
        )
        .await?;
        db.execute_unprepared(&format!(
            "CREATE UNIQUE INDEX {INDEX} ON mount_container \
             (location_type, location_id, lower(name)) WHERE retired_at IS NULL"
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name(INDEX)
                    .table(MountContainer::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MountContainer::Table)
                    .drop_column(MountContainer::RetiredAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum MountContainer {
    Table,
    RetiredAt,
}

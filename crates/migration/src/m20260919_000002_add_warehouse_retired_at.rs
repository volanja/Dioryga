//! `WAREHOUSE` に廃止（`retired_at`）を足す（#133）。
//!
//! 倉庫の削除には2つの結末がある。**一度も使われていない倉庫は行ごと消せる**が、
//! **過去に機器・部品を置いた倉庫は消せない。**所在の履歴
//! （`DEVICE_ASSIGNMENT` / `PART_INSTANCE_LOCATION`）が `location_type="Warehouse"`
//! で倉庫を指し続けており、**多態的参照のため外部キーが無く、DBは削除を止めない**
//! （24.5）。消すと履歴が参照先を失う。
//!
//! そこで後者は廃止として残す。**カタログの廃番（18.5）と同じ扱い**で、一覧の
//! 既定から隠し、新たな置き場の候補から外すが、履歴の表示には引き続き使う。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Warehouse::Table)
                    .add_column(timestamp_with_time_zone_null(Warehouse::RetiredAt))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Warehouse::Table)
                    .drop_column(Warehouse::RetiredAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Warehouse {
    Table,
    RetiredAt,
}

//! `VENDOR` と `PART_CATALOG` に統合先を足す（設計書18.5）。
//!
//! # なぜこの2つだけか
//!
//! **統合を作るのは、重複が集計を割るカタログに限る**（18.5-3）。
//! `CHASSIS_MODEL` は `CHASSIS_SLOT` の付け替えがスペックそのものの書き換えに
//! なり、`CONFIGURATION` は `DEVICE` 側の統合（23.9）で扱える。
//!
//! # 外部キーは張らない
//!
//! 既存テーブルへの**後からの列追加**であり、SQLiteは `ALTER TABLE` で制約を
//! 足せない（24.3）。`work_order_id` と同じ扱いとし、参照整合はアプリケーション
//! 層と `dioryga check`（24.5）で担保する。
//!
//! **`DEVICE.merged_into_device_id` はテーブル作成時に定義されているため事情が
//! 違う。**同じ役割の列で扱いが分かれるのは気持ちが悪いが、これはSQLiteの制約
//! であって設計の揺れではない。
//!
//! # 削除ではなく転送先の設定
//!
//! 23.9.4と同じ形。吸収された側の行は**削除しない**——`AUDIT_LOG.record_id`
//! が参照しており、物理削除すると監査記録が壊れる。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (table, merged_into) in 対象() {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(table))
                        .add_column(integer_null(Alias::new(merged_into)))
                        .to_owned(),
                )
                .await?;
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(table))
                        .add_column(timestamp_with_time_zone_null(MergedAt))
                        .to_owned(),
                )
                .await?;

            // **一覧の既定は「統合されていないもの」**（18.5、23.9.4）。
            // 全カタログで引く条件になるため索引を張る（24.3）
            manager
                .create_index(
                    Index::create()
                        .name(format!("idx_{table}_merged_into"))
                        .table(Alias::new(table))
                        .col(Alias::new(merged_into))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (table, merged_into) in 対象() {
            for column in [merged_into, "merged_at"] {
                manager
                    .alter_table(
                        Table::alter()
                            .table(Alias::new(table))
                            .drop_column(Alias::new(column))
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

fn 対象() -> [(&'static str, &'static str); 2] {
    [
        ("vendor", "merged_into_vendor_id"),
        ("part_catalog", "merged_into_part_catalog_id"),
    ]
}

#[derive(DeriveIden)]
struct MergedAt;

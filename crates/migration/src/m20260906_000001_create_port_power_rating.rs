//! `PORT_POWER_RATING` を作り、`PART_PORT_SLOT` の電圧列を移す（設計書12.7）。
//!
//! # なぜ子テーブルなのか
//!
//! **1つのポートが複数の給電方式を持ちうる。**Dellには`AC 100~240V`と`DC 240V`の
//! 双方を受けるPSUがあり、Ciscoには`-48V`のDC電源がある。12.7はこれを根拠に
//! **給電方式ごとに1行**を持つ形を選んだ。
//!
//! `m20260905_000005_add_port_voltage` は `PART_PORT_SLOT` に `voltage_min` /
//! `voltage_max` を直接足していた。**この形ではAC/DC両対応のPSUを表現できない**
//! ——`current_type` を持つ場所が無く、範囲も1つしか持てない。設計に合わせて
//! 作り直す。
//!
//! # 電圧は符号を含めてそのまま持つ
//!
//! Ciscoの`-48V`電源は許容範囲が`-40 to -72`であり、**絶対値が大きいほうが
//! `voltage_max` ではない。**`voltage_min ≤ voltage_max` という素直な大小関係で
//! 扱えるよう、-72を`voltage_min`、-40を`voltage_max`として格納する（12.7）。
//!
//! # 既存の行は符号で方式を判定して移す
//!
//! **`AC` に寄せない。**12.7がDCを負値で表すと決めている以上、負の電圧は
//! DCとしか解釈しようがなく、正の電圧をDCと解釈する余地も無い。既存の値から
//! 方式を復元できるため、ここでは推測ではなく判定になる。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PortPowerRating::Table)
                    .if_not_exists()
                    .col(pk_auto(PortPowerRating::Id))
                    .col(integer(PortPowerRating::PartPortSlotId))
                    .col(string(PortPowerRating::CurrentType))
                    .col(integer(PortPowerRating::VoltageMin))
                    .col(integer(PortPowerRating::VoltageMax))
                    .col(timestamp_with_time_zone(PortPowerRating::CreatedAt))
                    .col(timestamp_with_time_zone(PortPowerRating::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_port_power_rating_part_port_slot")
                            .from(PortPowerRating::Table, PortPowerRating::PartPortSlotId)
                            .to(PartPortSlot::Table, PartPortSlot::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // **方式ごとに1行**（12.7）。同じポートに`AC`を2行持つ意味は無い。
        // 外部キー列が先頭にあるため、`requireForeignKeyIndex` もこれで満たす
        manager
            .create_index(
                Index::create()
                    .name("idx_port_power_rating_slot_current")
                    .table(PortPowerRating::Table)
                    .col(PortPowerRating::PartPortSlotId)
                    .col(PortPowerRating::CurrentType)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // 既存の値を移す。**時刻は親の行から引き継ぐ**——`CURRENT_TIMESTAMP` は
        // 両DBで書式が異なり、SQLite側で読めない値が入りうる
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                INSERT INTO port_power_rating
                    (part_port_slot_id, current_type, voltage_min, voltage_max,
                     created_at, updated_at)
                SELECT id,
                       CASE WHEN voltage_min < 0 THEN 'DC' ELSE 'AC' END,
                       voltage_min,
                       voltage_max,
                       created_at,
                       updated_at
                FROM part_port_slot
                WHERE port_kind = 'Power'
                  AND voltage_min IS NOT NULL
                  AND voltage_max IS NOT NULL
                "#,
            )
            .await?;

        // **同じ意味の値を2箇所に残さない。**SQLiteも `DROP COLUMN` は扱える
        // （対応していないのは制約の追加である——24.3）
        for column in [PartPortSlot::VoltageMin, PartPortSlot::VoltageMax] {
            manager
                .alter_table(
                    Table::alter()
                        .table(PartPortSlot::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [PartPortSlot::VoltageMin, PartPortSlot::VoltageMax] {
            manager
                .alter_table(
                    Table::alter()
                        .table(PartPortSlot::Table)
                        .add_column(integer_null(column))
                        .to_owned(),
                )
                .await?;
        }

        // **戻せるのは1方式ぶんだけ。**両対応のポートは片方が落ちる——
        // それがこのマイグレーションを入れた理由そのものである
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                UPDATE part_port_slot SET
                    voltage_min = (SELECT MIN(r.voltage_min) FROM port_power_rating r
                                   WHERE r.part_port_slot_id = part_port_slot.id),
                    voltage_max = (SELECT MAX(r.voltage_max) FROM port_power_rating r
                                   WHERE r.part_port_slot_id = part_port_slot.id)
                "#,
            )
            .await?;

        manager
            .drop_table(Table::drop().table(PortPowerRating::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PortPowerRating {
    Table,
    Id,
    PartPortSlotId,
    CurrentType,
    VoltageMin,
    VoltageMax,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PartPortSlot {
    Table,
    Id,
    VoltageMin,
    VoltageMax,
}

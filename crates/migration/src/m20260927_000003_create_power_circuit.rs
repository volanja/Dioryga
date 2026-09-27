//! 設備・什器の給電（`POWER_CIRCUIT`）と設置場所を足す（#206、設計書12.10）。
//!
//! # 回路は設備・什器に付く
//!
//! 1つのラックには A系・B系のように複数の回路が来る。回路はデータセンターが
//! ラックへ引き込むものであり、PDU はそこに挿す。**12.7が PDU に付けていた
//! 親を設備・什器に移した**（12.10）。同じ型番のラックでも使い方が分かれる
//! ので、カタログではなく設備・什器ごとに持つ。
//!
//! **履歴テーブルである**（4章）。回路の200V化は実際に起きるため、変更は
//! 閉じて開く。電流はミリアンペアの整数（24.2.1）。
//!
//! # 設置場所は自由記述
//!
//! 置き場所（プロジェクト／倉庫）は所有者であって物理的な場所ではない。
//! データセンター・フロア・列を1つの列で持つ。並べ方が組織ごとに違うため、
//! 場所のマスタは作らない（12.10）。

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
                    .table(PowerCircuit::Table)
                    .if_not_exists()
                    .col(pk_auto(PowerCircuit::Id))
                    .col(integer(PowerCircuit::ContainerId))
                    .col(string(PowerCircuit::CircuitLabel))
                    .col(integer(PowerCircuit::Voltage))
                    // Single / Three。三相の計算はv2（12.7）だが列は持つ
                    .col(string(PowerCircuit::Phase))
                    .col(integer(PowerCircuit::BreakerCurrentMa))
                    // コンセントの形状。開いた語彙（8.6）
                    .col(string_null(PowerCircuit::ConnectorType))
                    .col(timestamp_with_time_zone(PowerCircuit::FromDate))
                    .col(timestamp_with_time_zone_null(PowerCircuit::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(PowerCircuit::Table, PowerCircuit::ContainerId)
                            .to(MountContainer::Table, MountContainer::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 外部キーの索引（24.3）。**現在の回路を引く**クエリが最も多い
        manager
            .create_index(
                Index::create()
                    .name("idx_power_circuit_container_id")
                    .table(PowerCircuit::Table)
                    .col(PowerCircuit::ContainerId)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(MountContainer::Table)
                    .add_column(string_null(MountContainer::InstallationSite))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MountContainer::Table)
                    .drop_column(MountContainer::InstallationSite)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(PowerCircuit::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PowerCircuit {
    Table,
    Id,
    ContainerId,
    CircuitLabel,
    Voltage,
    Phase,
    BreakerCurrentMa,
    ConnectorType,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum MountContainer {
    Table,
    Id,
    InstallationSite,
}

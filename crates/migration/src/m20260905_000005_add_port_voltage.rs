//! `PART_PORT_SLOT` に定格電圧の範囲を足す（設計書8.3、12.7）。
//!
//! **12.7が「v1で入れる」と明記した列である。**「v1で作らないのは画面と機能で
//! あってテーブル定義ではない」（16.6）の原則に加えて、12.7は次の区別を置いた。
//!
//! | 対象 | 判断 |
//! |---|---|
//! | v1で使うテーブルへの列・制約の追加 | **v1で入れる**（後から既定値や埋め戻しが要る） |
//! | v1で1行も入らない新規テーブル | v2でよい（作り直す既存データが無い） |
//!
//! `PART_PORT_SLOT` は既存のカタログテーブルであり、構成図からの取込がv1の対象に
//! 含まれる。カタログ画面を作る段で列が無いことに気付いたので、ここで入れる。
//!
//! # なぜ範囲で持つか
//!
//! 単一値では「100-240V対応」と「200V専用」を区別できない（12.7）。PSUの
//! インレットは `100`〜`240`、PDUのアウトレットは `200`〜`200` になる。
//!
//! # `port_kind=Power` のときだけ意味を持つ
//!
//! `port_speed` が `Network` のときだけ意味を持つのと同じ扱いである。**他の
//! `port_kind` に書かれた値は無視するが、黙って捨てず警告として記録する**
//! （23.6の「取り込むが記録する」）。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
}

#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum PartPortSlot {
    Table,
    VoltageMin,
    VoltageMax,
}

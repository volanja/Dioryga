//! `CONFIGURATION` に想定消費電力を足す（設計書12.8）。
//!
//! # カタログではなく設計値である
//!
//! **消費電力は型番から一意に決まらない。**Dellが構成図に消費電力を載せず
//! コンフィギュレーターで計算させているのはこのためで、HPEと富士通が載せている
//! 数値も「プロセッサー×2、メモリ×8、Utilization 100%」のような条件付きの
//! 参考値である。**カタログの列に入れてはならない**（12.8）。
//!
//! **使用電圧もカタログでは決まらない。**同じPSUでも100Vの回路に挿すか200Vかで
//! 引く電流が変わる（12.7）。よってこの3列は人が入力する設計値になる。
//!
//! # なぜ `CHASSIS_MODEL` ではないか
//!
//! 同じ筐体でもCPUとメモリの積み方で消費電力が変わる。6.1が `CONFIGURATION` を
//! 「性能要件ごとに異なるパーツの組み合わせ」と定義しているため、想定消費電力は
//! この単位に属する。
//!
//! # 電流を保存しない
//!
//! `A = VA ÷ V` で求める（不変条件2）。3つとも保存すると、**式が崩れた組み合わせを
//! 保存できてしまう。**
//!
//! # 皮相電力（VA）で持つ
//!
//! **ブレーカーに効くのは皮相電力であって有効電力（W）ではない**（12.7）。
//! ワット基準の集計は電圧と力率を落としており、「ブレーカー容量の設計」に対して
//! 精度が足りない。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // **いずれもnullable。**入力は任意であり、分からないものを0で埋めさせない。
        //
        // **1文につき1列。**SQLiteは `ALTER TABLE` に複数の操作を並べられない
        // （`Sqlite doesn't support multiple alter options`）
        manager
            .alter_table(
                Table::alter()
                    .table(Configuration::Table)
                    .add_column(string_null(Configuration::CurrentType))
                    .to_owned(),
            )
            .await?;
        for column in [Configuration::AssumedVoltage, Configuration::AssumedVa] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Configuration::Table)
                        .add_column(integer_null(column))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            Configuration::CurrentType,
            Configuration::AssumedVoltage,
            Configuration::AssumedVa,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Configuration::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Configuration {
    Table,
    CurrentType,
    AssumedVoltage,
    AssumedVa,
}

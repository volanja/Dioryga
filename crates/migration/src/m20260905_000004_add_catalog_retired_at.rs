//! 全カタログに `retired_at`（廃番）を足す（設計書18.5）。
//!
//! # なぜ要るか
//!
//! **これは重複のために作る仕組みではない。**「ベンダーが製造終了した型番を
//! 新規登録の候補から外したい」という**廃番の要求はもともと存在する**（18.5）。
//! 重複が生まれたときの実害のうち「以後も選ばれ続ける」ほうを止める手当てにも
//! なる、という順序である。
//!
//! # 参照済みでも設定できる
//!
//! 18.2が禁じているのは**スペックを定義するフィールドの編集**であり、
//! 選択可否はスペックではない。既存の参照は壊さず、過去の事実として残る。
//!
//! # 列名は既存に揃える
//!
//! `USER.disabled_at`・`PROJECT.archived_at`・`SBOM_IMPORT.superseded_at`・
//! `SOFTWARE_INSTANCE.retired_at` と同じ「いつそうなったか」の持ち方である。
//! ライセンスの失効を `retired_at` と呼んだ9.4.1の前例に揃えた。
//!
//! # 列の追加はSQLiteでも通る
//!
//! 24.3で「SQLiteの `ALTER TABLE` は制約の追加に対応していない」と書いたが、
//! `ADD COLUMN` は対応している。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// 18.5が挙げた7つのカタログ。
const カタログ: &[&str] = &[
    "vendor",
    "chassis_model",
    "part_catalog",
    "configuration",
    "cable_catalog",
    "software_catalog",
    "vlan",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in カタログ {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(*table))
                        .add_column(timestamp_with_time_zone_null(RetiredAt))
                        .to_owned(),
                )
                .await?;

            // **一覧の既定は「廃番でないもの」**（18.5）。全カタログで引く条件に
            // なるため索引を張る。両DBとも自動では張らない（24.3）
            manager
                .create_index(
                    Index::create()
                        .name(format!("idx_{table}_retired_at"))
                        .table(Alias::new(*table))
                        .col(RetiredAt)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in カタログ {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(*table))
                        .drop_column(RetiredAt)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
struct RetiredAt;

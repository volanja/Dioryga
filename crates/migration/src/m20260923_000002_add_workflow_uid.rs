//! `MILESTONE` と `WORK_ORDER` に `uid` と `external_id` を足す（#140・#141）。
//!
//! どちらも識別子を持たないまま設計していた。画面から作る限りは困らないが、
//! **取込には突合キーが要る。**無ければ同じファイルを2回流すたびに増え、
//! 宣言的な取込（設計書23.1）が成り立たない。`DEVICE` と同じ2列にして、
//! 突合の解決順序（`uid` → `external_id`）をそのまま使えるようにする。
//!
//! # 既存行の `uid` は `id` から作る
//!
//! **`NOT NULL` の列を既存の表に足すには既定値が要る。**空文字のまま全行に
//! 入れると一意索引が張れないため、`mi-<id>` / `wo-<id>` を入れてから張る。
//! **`uid` は採番したあと変えない値であり、何であるかは問わない。**以後に
//! 作られる行はアプリケーションがUUIDを採番する。
//!
//! # 既定値の空文字が列に残る
//!
//! **SQLiteは既定値を落とせない**（`ALTER TABLE` は `ADD COLUMN`・
//! `DROP COLUMN`・`RENAME` のみ。設計書24.3）。PostgreSQL側だけ落とすと
//! スキーマがDBごとに分岐する（24.2）ため、両方に残す。一意索引があるので
//! **`uid` を省いた挿入は2件目で必ず落ちる。**

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// （表, 既存行の `uid` の接頭辞）
const 対象: &[(&str, &str)] = &[("milestone", "mi"), ("work_order", "wo")];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (表, 接頭辞) in 対象 {
            // **1回の ALTER で2列足さない。**SQLiteは複数の変更を並べられない
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(*表))
                        .add_column(string(Col::Uid).default(""))
                        .to_owned(),
                )
                .await?;
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(*表))
                        .add_column(string_null(Col::ExternalId))
                        .to_owned(),
                )
                .await?;

            // **一意索引を張る前に埋める。**全行が空文字のままでは張れない
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "UPDATE {表} SET uid = '{接頭辞}-' || id WHERE uid = ''"
                ))
                .await?;

            manager
                .create_index(
                    Index::create()
                        .name(format!("idx_{表}_uid"))
                        .table(Alias::new(*表))
                        .col(Col::Uid)
                        .unique()
                        .to_owned(),
                )
                .await?;
            // 突合の2段目（23.2）。一意ではない——取込元が同じ番号を使い回すことがある
            manager
                .create_index(
                    Index::create()
                        .name(format!("idx_{表}_external_id"))
                        .table(Alias::new(*表))
                        .col(Col::ExternalId)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (表, _) in 対象 {
            manager
                .drop_index(
                    Index::drop()
                        .name(format!("idx_{表}_external_id"))
                        .table(Alias::new(*表))
                        .to_owned(),
                )
                .await?;
            manager
                .drop_index(
                    Index::drop()
                        .name(format!("idx_{表}_uid"))
                        .table(Alias::new(*表))
                        .to_owned(),
                )
                .await?;
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(*表))
                        .drop_column(Col::Uid)
                        .to_owned(),
                )
                .await?;
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(*表))
                        .drop_column(Col::ExternalId)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Col {
    Uid,
    ExternalId,
}

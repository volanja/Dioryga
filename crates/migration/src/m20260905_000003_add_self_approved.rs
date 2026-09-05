//! `WORK_ORDER_APPROVAL` に自己承認のフラグを足す（設計書11.4-9、22章R-2）。
//!
//! **#46（11章のスキーマ）で入れ忘れていた列である。**11.4-9は
//!
//! > 自己承認が行われた場合、`WORK_ORDER_APPROVAL` にその旨のフラグを立て、
//! > 15章の `AUDIT_LOG` にも通常の承認とは区別できる形で記録する
//!
//! としており、承認画面を作る段になって不足に気付いた。
//!
//! # 列の追加はSQLiteでも通る
//!
//! 24.3で「SQLiteの `ALTER TABLE` は制約の追加に対応していない」と書いたが、
//! **`ADD COLUMN` は対応している。**制約（外部キー・CHECK）を後から足せないだけで、
//! 既定値つきの列を足すのは両DBとも問題ない。
//!
//! # なぜ導出しないか
//!
//! `approver_id` が担当者と一致するかは、その場では計算できる。しかし
//! **11.4-9が保存を求めているのは「後からメンバーが増えた際に経緯を追える」
//! ようにするため**である。承認当時に他の承認者がいなかったという事実は、
//! 後から人が増えると再現できない。導出可能に見えて、実は時点情報である。

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
                    .table(WorkOrderApproval::Table)
                    .add_column(boolean(WorkOrderApproval::SelfApproved).default(false))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(WorkOrderApproval::Table)
                    .drop_column(WorkOrderApproval::SelfApproved)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum WorkOrderApproval {
    Table,
    SelfApproved,
}

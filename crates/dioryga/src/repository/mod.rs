//! リポジトリ層。
//!
//! **すべての書き込みをここに通し、`AUDIT_LOG` を同一トランザクション内に
//! 自動で記録する**（設計書15.2、24.4）。DBトリガーを使わないのは、
//! PostgreSQLとSQLiteで書き方が異なり両DB対応と相性が悪いため。
//!
//! # 生のコネクションで書けないようにする
//!
//! [`AuditedTx`] は `DatabaseTransaction` を包み、**読み取り用の参照のみを
//! [`AuditedTx::reader`] で公開する。**書き込みは [`AuditedTx::insert`] などの
//! メソッドを通すしかないため、監査ログの記録を「書き忘れる」ことがない。
//!
//! 呼び出し側の記述に依存させない、という24.4の要求をこの形で満たしている。

pub mod audit;

use chrono::Utc;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, DatabaseConnection, DatabaseTransaction, DbErr,
    EntityTrait, IntoActiveModel, ModelTrait, TransactionTrait,
};

use crate::repository::audit::{to_masked_json, Audited};

/// 変更を行った主体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// 画面またはAPIからの通常の操作。
    User(i32),

    /// 一括取込による変更（設計書23章）。
    ///
    /// **この主体による変更では行ごとの監査ログを書かない。**25万行の取込で
    /// 25万行の監査ログが生まれ、22.5で指摘した肥大問題を悪化させるため。
    /// 取込で作られた行はすべて同じ主体・時刻・`import_run_id` を持つので、
    /// 1行ごとに複製しても情報が増えない。追跡は `IMPORT_RUN` が担う（24.4）。
    Import { import_run_id: i32 },

    /// 作成される行自身が主体となる場合。
    ///
    /// 初回セットアップで最初のSystem Adminを作るときにのみ使う。監査ログの
    /// ために架空のシステムユーザーを作らず、作成された当人のIDを記録する
    /// （設計書20.8）。
    SelfCreated,
}

impl Actor {
    /// この主体による変更を監査ログに記録するか。
    fn records_audit(&self) -> bool {
        !matches!(self, Actor::Import { .. })
    }

    /// `AUDIT_LOG.import_run_id` に記録する値。
    fn import_run_id(&self) -> Option<i32> {
        match self {
            Actor::Import { import_run_id } => Some(*import_run_id),
            _ => None,
        }
    }
}

/// 監査ログの記録を伴うトランザクション。
pub struct AuditedTx {
    txn: DatabaseTransaction,
    actor: Actor,
}

impl AuditedTx {
    pub async fn begin(db: &DatabaseConnection, actor: Actor) -> Result<Self, DbErr> {
        Ok(Self {
            txn: db.begin().await?,
            actor,
        })
    }

    /// 取込を1つのトランザクションとして始める（設計書23.6、24.4）。
    ///
    /// **`IMPORT_RUN` を同じトランザクションの中で先に作り、以後の主体を
    /// その取込にする。**マニフェストは複数のエンティティにまたがり、後の
    /// エンティティが前のエンティティを参照する（機器とその搭載など）。
    /// **全体を1つのトランザクションに収めて初めて、**ドライランが同じファイルで
    /// 作るものを参照でき、反映が途中で止まったときに何も残らない。
    ///
    /// ドライランはこのトランザクションを `rollback` する。`IMPORT_RUN` も
    /// 一緒に消えるため、**取り込んでいない取込の記録は残らない。**
    ///
    /// `IMPORT_RUN` を監査ログを経ずに書くのは、取込による変更の追跡を
    /// **この行自体が担う**ためである（24.4）。取込の主体は行ごとの監査ログを
    /// 書かないので、経由しても何も記録されない。
    pub async fn begin_import(
        db: &DatabaseConnection,
        run: entity::import_run::ActiveModel,
    ) -> Result<(Self, entity::import_run::Model), DbErr> {
        let txn = db.begin().await?;
        let run = run.insert(&txn).await?;
        let tx = Self {
            txn,
            actor: Actor::Import {
                import_run_id: run.id,
            },
        };
        Ok((tx, run))
    }

    /// 取込の件数を `IMPORT_RUN` に書く。**反映の最後、コミットの直前に呼ぶ。**
    ///
    /// 件数は全エンティティを流し終えるまで決まらないため、先に作った行を
    /// 後から埋める。書き込み経路は [`Self::begin_import`] と同じ理由で直接。
    pub async fn record_import_counts(
        &self,
        run: &entity::import_run::Model,
        created: i32,
        updated: i32,
        warning: i32,
    ) -> Result<entity::import_run::Model, DbErr> {
        let mut active: entity::import_run::ActiveModel = run.clone().into();
        active.created_count = sea_orm::Set(created);
        active.updated_count = sea_orm::Set(updated);
        active.warning_count = sea_orm::Set(warning);
        active.update(&self.txn).await
    }

    /// 読み取り用のコネクション。
    ///
    /// **書き込みには使わないこと。**書き込みは本構造体のメソッドを通す
    /// （そうしないと監査ログが残らない）。
    pub fn reader(&self) -> &DatabaseTransaction {
        &self.txn
    }

    pub async fn commit(self) -> Result<(), DbErr> {
        self.txn.commit().await
    }

    pub async fn rollback(self) -> Result<(), DbErr> {
        self.txn.rollback().await
    }

    /// 行を追加し、監査ログに `insert` として記録する。
    pub async fn insert<A>(&self, model: A) -> Result<<A::Entity as EntityTrait>::Model, DbErr>
    where
        A: ActiveModelTrait + ActiveModelBehavior + Send,
        <A::Entity as EntityTrait>::Model: IntoActiveModel<A> + Audited,
    {
        let inserted = model.insert(&self.txn).await?;
        let after = to_masked_json(&inserted).map_err(to_db_err)?;

        self.write_audit(
            <<A::Entity as EntityTrait>::Model as Audited>::TABLE,
            inserted.audit_id(),
            "insert",
            None,
            Some(after),
        )
        .await?;

        Ok(inserted)
    }

    /// 行を更新し、監査ログに `update` として記録する。
    ///
    /// 変更前の状態は呼び出し側が渡す。更新のためにActiveModelを組み立てる時点で
    /// 呼び出し側は必ず現在の行を持っているため、ここで問い合わせを増やさない。
    pub async fn update<A>(
        &self,
        before: &<A::Entity as EntityTrait>::Model,
        model: A,
    ) -> Result<<A::Entity as EntityTrait>::Model, DbErr>
    where
        A: ActiveModelTrait + ActiveModelBehavior + Send,
        <A::Entity as EntityTrait>::Model: IntoActiveModel<A> + Audited,
    {
        let before_json = to_masked_json(before).map_err(to_db_err)?;
        let updated = model.update(&self.txn).await?;
        let after_json = to_masked_json(&updated).map_err(to_db_err)?;

        self.write_audit(
            <<A::Entity as EntityTrait>::Model as Audited>::TABLE,
            updated.audit_id(),
            "update",
            Some(before_json),
            Some(after_json),
        )
        .await?;

        Ok(updated)
    }

    /// 行を削除し、監査ログに `delete` として記録する。
    ///
    /// なお本設計では利用者・機器・カタログを物理削除しない（設計書20.11、23.9）。
    /// 使うのは、履歴を持たない中間テーブルの行を取り消す場合などに限られる。
    pub async fn delete<M>(&self, model: M) -> Result<(), DbErr>
    where
        M: ModelTrait + Audited + IntoActiveModel<<M::Entity as EntityTrait>::ActiveModel>,
        <M::Entity as EntityTrait>::ActiveModel: ActiveModelBehavior + Send,
    {
        let before = to_masked_json(&model).map_err(to_db_err)?;
        let table = M::TABLE;
        let id = model.audit_id();

        model.delete(&self.txn).await?;
        self.write_audit(table, id, "delete", Some(before), None)
            .await?;

        Ok(())
    }

    async fn write_audit(
        &self,
        table_name: &str,
        record_id: i32,
        action: &str,
        before_json: Option<String>,
        after_json: Option<String>,
    ) -> Result<(), DbErr> {
        if !self.actor.records_audit() {
            return Ok(());
        }

        let user_id = match self.actor {
            Actor::User(id) => id,
            // 作成された当人を主体とする（設計書20.8）
            Actor::SelfCreated => record_id,
            Actor::Import { .. } => unreachable!("records_audit() で除外済み"),
        };

        entity::audit_log::ActiveModel {
            user_id: sea_orm::Set(user_id),
            table_name: sea_orm::Set(table_name.to_owned()),
            record_id: sea_orm::Set(record_id),
            action: sea_orm::Set(action.to_owned()),
            before_json: sea_orm::Set(before_json),
            after_json: sea_orm::Set(after_json),
            import_run_id: sea_orm::Set(self.actor.import_run_id()),
            changed_at: sea_orm::Set(Utc::now()),
            ..Default::default()
        }
        .insert(&self.txn)
        .await?;

        Ok(())
    }
}

/// JSON化の失敗をDBエラーとして扱う。呼び出し側から見れば書き込みの失敗である。
fn to_db_err(e: serde_json::Error) -> DbErr {
    DbErr::Custom(format!("監査ログ用のJSON化に失敗しました: {e}"))
}

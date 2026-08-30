//! マイグレーション定義。
//!
//! 単一バイナリで配布する以上、配布先に `sea-orm-cli` を用意させることはできない。
//! ここで定義したものを `dioryga` バイナリへ埋め込み、`dioryga migrate` で適用する
//! （設計書24.1）。

pub use sea_orm_migration::prelude::*;

mod m20260830_000001_create_base_tables;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260830_000001_create_base_tables::Migration)]
    }
}

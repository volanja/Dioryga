//! マイグレーション定義。
//!
//! 単一バイナリで配布する以上、配布先に `sea-orm-cli` を用意させることはできない。
//! ここで定義したものを `dioryga` バイナリへ埋め込み、`dioryga migrate` で適用する
//! （設計書24.1）。

pub use sea_orm_migration::prelude::*;

mod m20260830_000001_create_base_tables;
mod m20260901_000001_create_catalog_tables;
mod m20260901_000002_create_device_tables;
mod m20260901_000003_create_placement_tables;
mod m20260902_000001_add_missing_fk_indexes;
mod m20260903_000001_create_import_run;
mod m20260903_000002_create_network_tables;
mod m20260903_000003_create_qcd_tables;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260830_000001_create_base_tables::Migration),
            Box::new(m20260901_000001_create_catalog_tables::Migration),
            Box::new(m20260901_000002_create_device_tables::Migration),
            Box::new(m20260901_000003_create_placement_tables::Migration),
            Box::new(m20260902_000001_add_missing_fk_indexes::Migration),
            Box::new(m20260903_000001_create_import_run::Migration),
            Box::new(m20260903_000002_create_network_tables::Migration),
            Box::new(m20260903_000003_create_qcd_tables::Migration),
        ]
    }
}

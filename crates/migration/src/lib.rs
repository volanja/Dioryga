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
mod m20260905_000001_create_work_order_tables;
mod m20260905_000002_create_software_tables;
mod m20260905_000003_add_self_approved;
mod m20260905_000004_add_catalog_retired_at;
mod m20260905_000005_add_port_voltage;
mod m20260905_000006_add_catalog_merge;
mod m20260906_000001_create_port_power_rating;
mod m20260906_000002_add_configuration_power;
mod m20260906_000003_add_cable_rating;
// **このマイグレーションだけ公開する。**結合テスト（`catalog_schema.rs`）が
// 名指しで up / down を流し、既存の行が書き換わることを確かめる。後から
// マイグレーションを足しても対象がずれない（#125、#120）
pub mod m20260916_000001_uppercase_device_category;
mod m20260919_000001_add_user_theme;
mod m20260919_000002_add_warehouse_retired_at;
pub mod m20260922_000001_rename_device_status;
pub mod m20260922_000002_rename_work_order_status;

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
            Box::new(m20260905_000001_create_work_order_tables::Migration),
            Box::new(m20260905_000002_create_software_tables::Migration),
            Box::new(m20260905_000003_add_self_approved::Migration),
            Box::new(m20260905_000004_add_catalog_retired_at::Migration),
            Box::new(m20260905_000005_add_port_voltage::Migration),
            Box::new(m20260905_000006_add_catalog_merge::Migration),
            Box::new(m20260906_000001_create_port_power_rating::Migration),
            Box::new(m20260906_000002_add_configuration_power::Migration),
            Box::new(m20260906_000003_add_cable_rating::Migration),
            Box::new(m20260916_000001_uppercase_device_category::Migration),
            Box::new(m20260919_000001_add_user_theme::Migration),
            Box::new(m20260919_000002_add_warehouse_retired_at::Migration),
            Box::new(m20260922_000001_rename_device_status::Migration),
            Box::new(m20260922_000002_rename_work_order_status::Migration),
        ]
    }
}

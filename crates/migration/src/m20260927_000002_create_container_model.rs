//! 設備・什器の型番を共有カタログ（`CONTAINER_MODEL`）に持たせる（#205、設計書12.10）。
//!
//! # 型番で決まるものはカタログに置く
//!
//! 種別と収容能力（RackのU数、Shelvingの段数）、寸法・重量・静荷重は型番で
//! 決まる。**`MOUNT_CONTAINER` の `container_type` と `capacity` は廃し、
//! 設備はカタログを指す。**型番で決まる値を両方に持つと食い違う。
//!
//! 重量・荷重はグラム、寸法はミリメートルの整数で持つ（24.2.1。最小単位を
//! 列名に含める）。`CHASSIS_MODEL` にも重量を足す——設備の静荷重と、載せた
//! 機器の重量の合計を比べるため。
//!
//! # 既存の設備・什器は消して作り直す
//!
//! 既存の行には指すべき型番が無い。仮のベンダーと型番をでっち上げて埋めると、
//! **実在しない行が共有カタログに残る。**未リリースなので行を失ってよい
//! （設計書16.6）。設備と、それを指す搭載の履歴・レンタル費を消す。
//!
//! # `container_model_id` はDB上は NULL を許す
//!
//! **SQLiteは、外部キーを持つ列を `ADD COLUMN` で足すとき既定値を NULL に
//! しか取れない。**表を作り直すと、`DEVICE_MOUNT` からの外部キーの張り直しが
//! 両DBで別の手順になる（24.2）。必須であることはアプリケーション層が保証する。

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
                    .table(ContainerModel::Table)
                    .if_not_exists()
                    .col(pk_auto(ContainerModel::Id))
                    .col(integer(ContainerModel::VendorId))
                    .col(string(ContainerModel::ModelName))
                    .col(string(ContainerModel::ContainerType))
                    // 種別で使う列が分かれる（Rackは height_u、Shelvingは shelf_count）
                    .col(integer_null(ContainerModel::HeightU))
                    .col(integer_null(ContainerModel::ShelfCount))
                    .col(integer_null(ContainerModel::WidthMm))
                    .col(integer_null(ContainerModel::DepthMm))
                    .col(integer_null(ContainerModel::HeightMm))
                    .col(integer_null(ContainerModel::WeightG))
                    .col(integer_null(ContainerModel::StaticLoadG))
                    .col(timestamp_with_time_zone_null(ContainerModel::RetiredAt))
                    .col(integer(ContainerModel::CreatedBy))
                    .col(timestamp_with_time_zone(ContainerModel::CreatedAt))
                    .col(timestamp_with_time_zone(ContainerModel::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ContainerModel::Table, ContainerModel::VendorId)
                            .to(Vendor::Table, Vendor::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ContainerModel::Table, ContainerModel::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 自然キー（`CHASSIS_MODEL` と同じ「ベンダー＋型番」）。外部キーの索引も兼ねる
        manager
            .create_index(
                Index::create()
                    .name("uq_container_model_vendor_model_name")
                    .table(ContainerModel::Table)
                    .col(ContainerModel::VendorId)
                    .col(ContainerModel::ModelName)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(ChassisModel::Table)
                    .add_column(integer_null(ChassisModel::WeightG))
                    .to_owned(),
            )
            .await?;

        // **既存の設備・什器は消す。**指している行から先に消す
        let db = manager.get_connection();
        db.execute_unprepared("DELETE FROM device_mount WHERE container_id IS NOT NULL")
            .await?;
        db.execute_unprepared("DELETE FROM recurring_cost WHERE item_type = 'MountContainer'")
            .await?;
        db.execute_unprepared("DELETE FROM mount_container").await?;

        // **列と外部キーは同じ文で足す。**SQLiteは ALTER TABLE で外部キーを後から
        // 足せず、1回の ALTER に複数の変更も並べられない。`ADD COLUMN ... REFERENCES`
        // なら両DBで通る
        db.execute_unprepared(
            "ALTER TABLE mount_container \
             ADD COLUMN container_model_id INTEGER REFERENCES container_model (id)",
        )
        .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_mount_container_container_model_id")
                    .table(MountContainer::Table)
                    .col(MountContainer::ContainerModelId)
                    .to_owned(),
            )
            .await?;

        // **1回の ALTER で2列落とさない。**SQLiteは複数の変更を並べられない
        for col in [MountContainer::ContainerType, MountContainer::Capacity] {
            manager
                .alter_table(
                    Table::alter()
                        .table(MountContainer::Table)
                        .drop_column(col)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // 消した設備・什器は戻せない。未リリースの作り直しとして、戻す経路は持たない
        Err(DbErr::Migration(
            "設備・什器のカタログ化（#205）は戻せません".to_owned(),
        ))
    }
}

#[derive(DeriveIden)]
enum ContainerModel {
    Table,
    Id,
    VendorId,
    ModelName,
    ContainerType,
    HeightU,
    ShelfCount,
    WidthMm,
    DepthMm,
    HeightMm,
    WeightG,
    StaticLoadG,
    RetiredAt,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum MountContainer {
    Table,
    ContainerModelId,
    ContainerType,
    Capacity,
}

#[derive(DeriveIden)]
enum ChassisModel {
    Table,
    WeightG,
}

#[derive(DeriveIden)]
enum Vendor {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
}

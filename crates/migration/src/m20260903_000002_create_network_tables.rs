//! ネットワークのテーブルを作成する（設計書8章、14章）。
//!
//! # OSから見た姿を持つ
//!
//! 8.3で「構成図そのままではなく、実務上何が重要か」で再設計した結果、中心に
//! 置いたのは物理ポートではなく **`OS_INTERFACE`**（OSから見えるインター
//! フェース）である。ボンド・VLANサブインターフェース・SVI・VMの仮想NICは
//! 物理ポートに1対1で対応しない。
//!
//! # `UNIQUE(subnet_id, ip_address) WHERE to_date IS NULL` はDB制約にする
//!
//! **本設計では珍しく、業務ルールをDBで担保する。**部分インデックスは
//! PostgreSQL・SQLiteのどちらも対応しており（14.2）、DBごとの分岐が起きない。
//! IPの重複は「あとで直せばよい」類の誤りではないため、入口で止める。
//!
//! # `work_order_id` にはまだ外部キーを張らない
//!
//! `WORK_ORDER`（11章）が未作成のため列だけを置く。**テーブルを作る際に
//! 外部キーを追加すること。**機器・配置の各テーブルと同じ状態である。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // -------------------------------------------------------------------
        // VLAN / SUBNET（8.3、14.1）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Vlan::Table)
                    .if_not_exists()
                    .col(pk_auto(Vlan::Id))
                    .col(integer(Vlan::VlanTag))
                    .col(string(Vlan::Name).default(""))
                    // セキュリティ境界（8.5）。SUBNET側にも持つ
                    .col(string_null(Vlan::Zone))
                    .col(string(Vlan::Description).default(""))
                    .col(integer(Vlan::CreatedBy))
                    .col(timestamp_with_time_zone(Vlan::CreatedAt))
                    .col(timestamp_with_time_zone(Vlan::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Vlan::Table, Vlan::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // **一意制約は張らない。**VLANタグはL2ドメインごとに独立しており、
        // 拠点が違えば同じ 100 を使える。設計書8.3もVLANに一意制約を定めて
        // いない。索引は検索のためだけに張る
        index(manager, "idx_vlan_tag", Vlan::Table, Vlan::VlanTag).await?;
        index(manager, "idx_vlan_created_by", Vlan::Table, Vlan::CreatedBy).await?;

        manager
            .create_table(
                Table::create()
                    .table(Subnet::Table)
                    .if_not_exists()
                    .col(pk_auto(Subnet::Id))
                    // **プロジェクト単位に分ける。**異なるプロジェクトが同じ
                    // プライベートアドレス帯を独立に使っていても衝突しない（14.2）
                    .col(integer_null(Subnet::ProjectId))
                    .col(integer_null(Subnet::VlanId))
                    .col(string(Subnet::Cidr))
                    // VLANを伴わないサブネットがあるため、こちらにもzoneを持つ。
                    // 両方あるときは SUBNET.zone を優先する（14.2）
                    .col(string_null(Subnet::Zone))
                    .col(string(Subnet::Description).default(""))
                    .col(integer(Subnet::CreatedBy))
                    .col(timestamp_with_time_zone(Subnet::CreatedAt))
                    .col(timestamp_with_time_zone(Subnet::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Subnet::Table, Subnet::ProjectId)
                            .to(Project::Table, Project::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Subnet::Table, Subnet::VlanId)
                            .to(Vlan::Table, Vlan::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Subnet::Table, Subnet::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_subnet_project",
            Subnet::Table,
            Subnet::ProjectId,
        )
        .await?;
        index(manager, "idx_subnet_vlan", Subnet::Table, Subnet::VlanId).await?;
        index(
            manager,
            "idx_subnet_created_by",
            Subnet::Table,
            Subnet::CreatedBy,
        )
        .await?;

        // -------------------------------------------------------------------
        // OS_INTERFACE（8.3、8.5）— 履歴
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(OsInterface::Table)
                    .if_not_exists()
                    .col(pk_auto(OsInterface::Id))
                    // **必須。**ボンド・VLANサブインターフェース・SVIは物理ポートを
                    // 持たず、ポートから機器を導出できない（8.3）。nullableに
                    // すると、これらが宙に浮く
                    .col(integer(OsInterface::DeviceId))
                    .col(string(OsInterface::InterfaceType))
                    // interface_type=Physical のときのみ意味を持つ
                    .col(integer_null(OsInterface::PartInstanceId))
                    .col(integer_null(OsInterface::PortSlotId))
                    // ens1f0, bond0, bond0.100, Eth1/1/1, Vlan100
                    .col(string(OsInterface::OsInterfaceName))
                    // LACP / Static / ActiveBackup。interface_type=Bond のみ
                    .col(string_null(OsInterface::AggregationMode))
                    .col(integer_null(OsInterface::WorkOrderId))
                    .col(timestamp_with_time_zone(OsInterface::FromDate))
                    .col(timestamp_with_time_zone_null(OsInterface::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(OsInterface::Table, OsInterface::DeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(OsInterface::Table, OsInterface::PartInstanceId)
                            .to(PartInstance::Table, PartInstance::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(OsInterface::Table, OsInterface::PortSlotId)
                            .to(PartPortSlot::Table, PartPortSlot::Id),
                    )
                    .to_owned(),
            )
            .await?;

        current_index(
            manager,
            "idx_os_interface_device_current",
            OsInterface::Table,
            OsInterface::DeviceId,
            OsInterface::ToDate,
        )
        .await?;
        // 「このポートに何が生えているか」を引く（機器詳細・配線）
        current_index(
            manager,
            "idx_os_interface_port_slot_current",
            OsInterface::Table,
            OsInterface::PortSlotId,
            OsInterface::ToDate,
        )
        .await?;
        index(
            manager,
            "idx_os_interface_part_instance",
            OsInterface::Table,
            OsInterface::PartInstanceId,
        )
        .await?;

        // -------------------------------------------------------------------
        // INTERFACE_STACK（8.5）— 履歴
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(InterfaceStack::Table)
                    .if_not_exists()
                    .col(pk_auto(InterfaceStack::Id))
                    .col(integer(InterfaceStack::UpperInterfaceId))
                    .col(integer(InterfaceStack::LowerInterfaceId))
                    .col(timestamp_with_time_zone(InterfaceStack::FromDate))
                    .col(timestamp_with_time_zone_null(InterfaceStack::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(InterfaceStack::Table, InterfaceStack::UpperInterfaceId)
                            .to(OsInterface::Table, OsInterface::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(InterfaceStack::Table, InterfaceStack::LowerInterfaceId)
                            .to(OsInterface::Table, OsInterface::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // **双方向に引く。**ボンドは 1 upper : N lower、VLANサブインター
        // フェースは 1 lower : N upper であり、どちらからも辿る（8.5）
        current_index(
            manager,
            "idx_interface_stack_upper_current",
            InterfaceStack::Table,
            InterfaceStack::UpperInterfaceId,
            InterfaceStack::ToDate,
        )
        .await?;
        current_index(
            manager,
            "idx_interface_stack_lower_current",
            InterfaceStack::Table,
            InterfaceStack::LowerInterfaceId,
            InterfaceStack::ToDate,
        )
        .await?;

        // -------------------------------------------------------------------
        // INTERFACE_VLAN / INTERFACE_ROLE（8.3、8.5、8.6）— 履歴
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(InterfaceVlan::Table)
                    .if_not_exists()
                    .col(pk_auto(InterfaceVlan::Id))
                    .col(integer(InterfaceVlan::OsInterfaceId))
                    .col(integer(InterfaceVlan::VlanId))
                    // Untagged（ポートVLAN）/ Tagged（タグVLAN）
                    .col(string(InterfaceVlan::TaggingMode))
                    .col(integer_null(InterfaceVlan::WorkOrderId))
                    .col(timestamp_with_time_zone(InterfaceVlan::FromDate))
                    .col(timestamp_with_time_zone_null(InterfaceVlan::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(InterfaceVlan::Table, InterfaceVlan::OsInterfaceId)
                            .to(OsInterface::Table, OsInterface::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(InterfaceVlan::Table, InterfaceVlan::VlanId)
                            .to(Vlan::Table, Vlan::Id),
                    )
                    .to_owned(),
            )
            .await?;

        current_index(
            manager,
            "idx_interface_vlan_interface_current",
            InterfaceVlan::Table,
            InterfaceVlan::OsInterfaceId,
            InterfaceVlan::ToDate,
        )
        .await?;
        // 「このVLANはどこで使われているか」を引く
        current_index(
            manager,
            "idx_interface_vlan_vlan_current",
            InterfaceVlan::Table,
            InterfaceVlan::VlanId,
            InterfaceVlan::ToDate,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(InterfaceRole::Table)
                    .if_not_exists()
                    .col(pk_auto(InterfaceRole::Id))
                    .col(integer(InterfaceRole::OsInterfaceId))
                    // Backup / vMotion / Management など、目的別の区別（8.5）
                    .col(string(InterfaceRole::Role))
                    .col(timestamp_with_time_zone(InterfaceRole::FromDate))
                    .col(timestamp_with_time_zone_null(InterfaceRole::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(InterfaceRole::Table, InterfaceRole::OsInterfaceId)
                            .to(OsInterface::Table, OsInterface::Id),
                    )
                    .to_owned(),
            )
            .await?;

        current_index(
            manager,
            "idx_interface_role_interface_current",
            InterfaceRole::Table,
            InterfaceRole::OsInterfaceId,
            InterfaceRole::ToDate,
        )
        .await?;

        // -------------------------------------------------------------------
        // IP_ADDRESS（8.3、8.6、14章）— 履歴
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(IpAddress::Table)
                    .if_not_exists()
                    .col(pk_auto(IpAddress::Id))
                    .col(integer(IpAddress::OsInterfaceId))
                    .col(string(IpAddress::IpAddress))
                    .col(integer(IpAddress::PrefixLength))
                    .col(integer_null(IpAddress::SubnetId))
                    .col(integer_null(IpAddress::WorkOrderId))
                    .col(timestamp_with_time_zone(IpAddress::FromDate))
                    .col(timestamp_with_time_zone_null(IpAddress::ToDate))
                    // **`vlan_id` を持たない。**インターフェース側から辿る（8.6で廃止）
                    .foreign_key(
                        ForeignKey::create()
                            .from(IpAddress::Table, IpAddress::OsInterfaceId)
                            .to(OsInterface::Table, OsInterface::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(IpAddress::Table, IpAddress::SubnetId)
                            .to(Subnet::Table, Subnet::Id),
                    )
                    .to_owned(),
            )
            .await?;

        current_index(
            manager,
            "idx_ip_address_interface_current",
            IpAddress::Table,
            IpAddress::OsInterfaceId,
            IpAddress::ToDate,
        )
        .await?;

        // **同一サブネット内でIPは重複できない**（14.2）。
        //
        // 本設計では業務ルールをDB制約にしないのが原則だが、ここは例外。
        // 部分インデックスは両DBが対応しており分岐が起きず、IPの重複は
        // 「あとで直せばよい」類の誤りではないため入口で止める。
        //
        // `subnet_id` が null の行は対象外になる（NULL同士は重複と見なされない）。
        // サブネット未登録のIPまでは守れないが、それは本来登録すべきという話。
        manager
            .create_index(
                Index::create()
                    .name("uq_ip_address_subnet_current")
                    .table(IpAddress::Table)
                    .col(IpAddress::SubnetId)
                    .col(IpAddress::IpAddress)
                    .and_where(Expr::col(IpAddress::ToDate).is_null())
                    .unique()
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // CABLE_INSTANCE / CABLE_CONNECTION（8.3）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(CableInstance::Table)
                    .if_not_exists()
                    .col(pk_auto(CableInstance::Id))
                    .col(integer(CableInstance::CableCatalogId))
                    .col(string_null(CableInstance::SerialNumber))
                    .col(string_null(CableInstance::AssetNumber))
                    // in_stock / in_use / broken / disposed。**機器と違い、
                    // ケーブルは所在の履歴を持たないため status に含む**
                    .col(string(CableInstance::Status))
                    .col(timestamp_with_time_zone(CableInstance::CreatedAt))
                    .col(timestamp_with_time_zone(CableInstance::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(CableInstance::Table, CableInstance::CableCatalogId)
                            .to(CableCatalog::Table, CableCatalog::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_cable_instance_catalog",
            CableInstance::Table,
            CableInstance::CableCatalogId,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(CableConnection::Table)
                    .if_not_exists()
                    .col(pk_auto(CableConnection::Id))
                    .col(integer(CableConnection::CableInstanceId))
                    // どちらの端か。両端が非対称なケーブルがあるため端ごとに持つ（8.7）
                    .col(integer(CableConnection::CableEndSlotId))
                    .col(integer(CableConnection::PartInstanceId))
                    .col(integer(CableConnection::PortSlotId))
                    .col(integer_null(CableConnection::WorkOrderId))
                    .col(timestamp_with_time_zone(CableConnection::FromDate))
                    .col(timestamp_with_time_zone_null(CableConnection::ToDate))
                    // **`device_id` を持たない。**PART_INSTANCE_LOCATION 経由で
                    // 導出する（8.3）。持つと部品の移設時に二重管理になる
                    .foreign_key(
                        ForeignKey::create()
                            .from(CableConnection::Table, CableConnection::CableInstanceId)
                            .to(CableInstance::Table, CableInstance::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(CableConnection::Table, CableConnection::CableEndSlotId)
                            .to(CableEndSlot::Table, CableEndSlot::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(CableConnection::Table, CableConnection::PartInstanceId)
                            .to(PartInstance::Table, PartInstance::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(CableConnection::Table, CableConnection::PortSlotId)
                            .to(PartPortSlot::Table, PartPortSlot::Id),
                    )
                    .to_owned(),
            )
            .await?;

        current_index(
            manager,
            "idx_cable_connection_cable_current",
            CableConnection::Table,
            CableConnection::CableInstanceId,
            CableConnection::ToDate,
        )
        .await?;
        // 「このポートに何が刺さっているか」を引く（配線図・機器詳細）
        current_index(
            manager,
            "idx_cable_connection_port_current",
            CableConnection::Table,
            CableConnection::PortSlotId,
            CableConnection::ToDate,
        )
        .await?;
        index(
            manager,
            "idx_cable_connection_end_slot",
            CableConnection::Table,
            CableConnection::CableEndSlotId,
        )
        .await?;
        index(
            manager,
            "idx_cable_connection_part_instance",
            CableConnection::Table,
            CableConnection::PartInstanceId,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            CableConnection::Table.into_iden(),
            CableInstance::Table.into_iden(),
            IpAddress::Table.into_iden(),
            InterfaceRole::Table.into_iden(),
            InterfaceVlan::Table.into_iden(),
            InterfaceStack::Table.into_iden(),
            OsInterface::Table.into_iden(),
            Subnet::Table.into_iden(),
            Vlan::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }
}

/// 外部キー列などへの通常のインデックス。両DBとも自動では張らない（24.3）。
async fn index<T, C>(
    manager: &SchemaManager<'_>,
    name: &str,
    table: T,
    column: C,
) -> Result<(), DbErr>
where
    T: IntoIden + 'static,
    C: IntoIden + 'static,
{
    manager
        .create_index(
            Index::create()
                .name(name)
                .table(table)
                .col(column)
                .to_owned(),
        )
        .await
}

/// 履歴テーブルの「現在有効な行」を引くための部分インデックス（24.3）。
async fn current_index<T, C, D>(
    manager: &SchemaManager<'_>,
    name: &str,
    table: T,
    column: C,
    to_date: D,
) -> Result<(), DbErr>
where
    T: IntoIden + 'static,
    C: IntoIden + 'static,
    D: IntoIden + 'static,
{
    manager
        .create_index(
            Index::create()
                .name(name)
                .table(table)
                .col(column)
                .and_where(Expr::col(to_date).is_null())
                .to_owned(),
        )
        .await
}

// --- 既存のマイグレーションで作成済み。外部キーの参照先としてのみ使う ---

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Project {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Device {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum PartInstance {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum PartPortSlot {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum CableCatalog {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum CableEndSlot {
    Table,
    Id,
}

// --- 本マイグレーションで作成する ---

/// 列名をそのまま識別子にするため `VlanTag` の接頭辞が重なる。
/// 改名するとDBの列名が変わるため、lintの側を黙らせる。
#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum Vlan {
    Table,
    Id,
    VlanTag,
    Name,
    Zone,
    Description,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Subnet {
    Table,
    Id,
    ProjectId,
    VlanId,
    Cidr,
    Zone,
    Description,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

/// 列名をそのまま識別子にするため接頭辞が重なる。改名するとDBの列名が変わる。
#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum OsInterface {
    Table,
    Id,
    DeviceId,
    InterfaceType,
    PartInstanceId,
    PortSlotId,
    OsInterfaceName,
    AggregationMode,
    WorkOrderId,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum InterfaceStack {
    Table,
    Id,
    UpperInterfaceId,
    LowerInterfaceId,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum InterfaceVlan {
    Table,
    Id,
    OsInterfaceId,
    VlanId,
    TaggingMode,
    WorkOrderId,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum InterfaceRole {
    Table,
    Id,
    OsInterfaceId,
    Role,
    FromDate,
    ToDate,
}

/// 列名 `ip_address` がテーブル名と重なる。DBの列名を変えないため許容する。
#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum IpAddress {
    Table,
    Id,
    OsInterfaceId,
    IpAddress,
    PrefixLength,
    SubnetId,
    WorkOrderId,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum CableInstance {
    Table,
    Id,
    CableCatalogId,
    SerialNumber,
    AssetNumber,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum CableConnection {
    Table,
    Id,
    CableInstanceId,
    CableEndSlotId,
    PartInstanceId,
    PortSlotId,
    WorkOrderId,
    FromDate,
    ToDate,
}

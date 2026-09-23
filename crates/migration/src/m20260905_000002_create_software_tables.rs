//! ソフトウェア・SBOMのテーブルを作成する（設計書9章）。
//!
//! # 量が3桁違う2種類のデータを別の機構で扱う
//!
//! 9.2で層を分けた。**境界は「商用かOSSか」ではなく「どう登録されたか」**で
//! 引く。
//!
//! | 層 | 対象 | 格納 |
//! |---|---|---|
//! | 管理対象ソフトウェア | 人が資産として登録したもの。1台あたり数件 | 関係テーブル |
//! | SBOM観測記録 | 取込で観測された全件。1台あたり数千件 | 圧縮スナップショット＋変更レコード |
//!
//! 10,000台規模（17.2）では後者が10,000×数千のオーダーになる。関係テーブルで
//! 持つと、**ライセンスキーも資産番号も持たない行を300万件作ることになる。**
//!
//! # SBOM_SNAPSHOT だけ代理キーを持たない
//!
//! 主キーが `content_hash`（正規化後の内容のSHA-256）である。他の全テーブルと
//! 異なる形だが、**これは意図的**（9.7）。サーバ群はゴールデンイメージから
//! 構築されるため同一構成の機器のSBOMは完全に一致し、内容を主キーにすれば
//! **保存されるblobはイメージの種類数に比例し、台数には比例しない。**
//!
//! `content` は正規化済みのコンポーネント一覧をzstdで圧縮したもの。内側の形式は
//! JSONのままとし、デバッグ容易性を優先している（9.7）。
//!
//! # SBOM_COMPONENT_INDEX は content_hash 単位に張る
//!
//! **Device単位にしない**（9.8）。Device単位だと3,000万行になるが、スナップ
//! ショットを共有している以上その必要がなく、イメージ種類数に比例する
//! 約60,000行で済む。
//!
//! **真実の源は `SBOM_SNAPSHOT` であり、この索引はいつでも捨てて再構築できる
//! 派生物である。**不整合はデータ損失にならない。
//!
//! # superseded_at は to_date と同じ役割
//!
//! `SBOM_IMPORT` は `from_date`/`to_date` ではなく `superseded_at` を使うが、
//! 役割は同じ（9.5）。「そのDeviceの現在の観測結果」は `superseded_at IS NULL`
//! で引ける。部分ユニークの張り方も他の履歴テーブルと揃えた。
//!
//! # SOFTWARE_INSTANCE は status を持たない
//!
//! 9.4.1。「インストールされているか」は `SOFTWARE_INSTALLATION` の現行行から
//! 導出する。ハードウェアの `failed`/`repairing` はライセンスに対応物がなく、
//! 稼働中サービスの異常は監視ツールの領域でスコープ外（1.2）。導出できない
//! 「まだ保有しているか」だけを `retired_at` で持つ。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // -------------------------------------------------------------------
        // 管理対象ソフトウェア
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(SoftwareInstance::Table)
                    .if_not_exists()
                    .col(pk_auto(SoftwareInstance::Id))
                    .col(integer(SoftwareInstance::SoftwareCatalogId))
                    .col(string_null(SoftwareInstance::LicenseKey))
                    .col(string_null(SoftwareInstance::AssetNumber))
                    // **`status` を持たない**（9.4.1）。null = 保有中
                    .col(timestamp_with_time_zone_null(SoftwareInstance::RetiredAt))
                    .col(timestamp_with_time_zone(SoftwareInstance::CreatedAt))
                    .col(timestamp_with_time_zone(SoftwareInstance::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(SoftwareInstance::Table, SoftwareInstance::SoftwareCatalogId)
                            .to(SoftwareCatalog::Table, SoftwareCatalog::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_software_instance_catalog",
            SoftwareInstance::Table,
            SoftwareInstance::SoftwareCatalogId,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(SoftwareInstallation::Table)
                    .if_not_exists()
                    .col(pk_auto(SoftwareInstallation::Id))
                    .col(integer(SoftwareInstallation::SoftwareInstanceId))
                    .col(integer(SoftwareInstallation::DeviceId))
                    // WORK_ORDER は作成済みのため、ここは外部キーを張れる。
                    // 先行して作った履歴テーブルの `work_order_id` は張れない（24.3）
                    .col(integer_null(SoftwareInstallation::WorkOrderId))
                    .col(timestamp_with_time_zone(SoftwareInstallation::FromDate))
                    .col(timestamp_with_time_zone_null(SoftwareInstallation::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                SoftwareInstallation::Table,
                                SoftwareInstallation::SoftwareInstanceId,
                            )
                            .to(SoftwareInstance::Table, SoftwareInstance::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(SoftwareInstallation::Table, SoftwareInstallation::DeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                SoftwareInstallation::Table,
                                SoftwareInstallation::WorkOrderId,
                            )
                            .to(WorkOrder::Table, WorkOrder::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_software_installation_instance",
            SoftwareInstallation::Table,
            SoftwareInstallation::SoftwareInstanceId,
        )
        .await?;
        index(
            manager,
            "idx_software_installation_device",
            SoftwareInstallation::Table,
            SoftwareInstallation::DeviceId,
        )
        .await?;
        index(
            manager,
            "idx_software_installation_work_order",
            SoftwareInstallation::Table,
            SoftwareInstallation::WorkOrderId,
        )
        .await?;

        // **1つのインスタンスが同時に2台へ入ることはない。**
        // ライセンスの実体は1つであり、移設は「閉じて開く」で表す（不変条件1）
        manager
            .create_index(
                Index::create()
                    .name("uq_software_installation_current")
                    .table(SoftwareInstallation::Table)
                    .col(SoftwareInstallation::SoftwareInstanceId)
                    .and_where(Expr::col(SoftwareInstallation::ToDate).is_null())
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SoftwareRoleAssignment::Table)
                    .if_not_exists()
                    .col(pk_auto(SoftwareRoleAssignment::Id))
                    .col(integer(SoftwareRoleAssignment::SoftwareInstallationId))
                    // OS / DNS / NTP / WebServer / App ...
                    // **SBOM取込の対象外。**常に運用者が画面で設定する（9.9）
                    .col(string(SoftwareRoleAssignment::Role))
                    .col(timestamp_with_time_zone(SoftwareRoleAssignment::FromDate))
                    .col(timestamp_with_time_zone_null(
                        SoftwareRoleAssignment::ToDate,
                    ))
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                SoftwareRoleAssignment::Table,
                                SoftwareRoleAssignment::SoftwareInstallationId,
                            )
                            .to(SoftwareInstallation::Table, SoftwareInstallation::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_software_role_assignment_installation",
            SoftwareRoleAssignment::Table,
            SoftwareRoleAssignment::SoftwareInstallationId,
        )
        .await?;

        // **同じroleを二重に付けない。**ただし別のroleは何件でも付く（9.9）
        manager
            .create_index(
                Index::create()
                    .name("uq_software_role_assignment_current")
                    .table(SoftwareRoleAssignment::Table)
                    .col(SoftwareRoleAssignment::SoftwareInstallationId)
                    .col(SoftwareRoleAssignment::Role)
                    .and_where(Expr::col(SoftwareRoleAssignment::ToDate).is_null())
                    .unique()
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // SBOM観測記録
        // -------------------------------------------------------------------

        // **代理キーを持たない唯一のテーブル。**内容のハッシュが主キーであり、
        // 同一内容は1件しか保存されない（9.7）
        manager
            .create_table(
                Table::create()
                    .table(SbomSnapshot::Table)
                    .if_not_exists()
                    .col(string(SbomSnapshot::ContentHash).primary_key())
                    // 正規化済みコンポーネント一覧をzstdで圧縮したもの
                    .col(blob(SbomSnapshot::Content))
                    .col(integer(SbomSnapshot::ComponentCount))
                    .col(timestamp_with_time_zone(SbomSnapshot::FirstSeenAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SbomImport::Table)
                    .if_not_exists()
                    .col(pk_auto(SbomImport::Id))
                    .col(integer(SbomImport::DeviceId))
                    .col(string(SbomImport::ContentHash))
                    // CycloneDX / SPDX
                    .col(string(SbomImport::SourceFormat))
                    .col(integer_null(SbomImport::WorkOrderId))
                    .col(integer(SbomImport::ImportedBy))
                    .col(timestamp_with_time_zone(SbomImport::ImportedAt))
                    // null = そのDeviceの最新の観測結果。`to_date` と同じ役割（9.5）
                    .col(timestamp_with_time_zone_null(SbomImport::SupersededAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(SbomImport::Table, SbomImport::DeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(SbomImport::Table, SbomImport::ContentHash)
                            .to(SbomSnapshot::Table, SbomSnapshot::ContentHash),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(SbomImport::Table, SbomImport::WorkOrderId)
                            .to(WorkOrder::Table, WorkOrder::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(SbomImport::Table, SbomImport::ImportedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_sbom_import_device",
            SbomImport::Table,
            SbomImport::DeviceId,
        )
        .await?;
        // 9.8の横断検索が `sbom_import` から `sbom_component_index` へ結合する。
        // 結合キーであり、索引が無いと10,000台の全走査になる
        index(
            manager,
            "idx_sbom_import_content_hash",
            SbomImport::Table,
            SbomImport::ContentHash,
        )
        .await?;
        index(
            manager,
            "idx_sbom_import_work_order",
            SbomImport::Table,
            SbomImport::WorkOrderId,
        )
        .await?;
        index(
            manager,
            "idx_sbom_import_imported_by",
            SbomImport::Table,
            SbomImport::ImportedBy,
        )
        .await?;

        // **1台につき最新の観測は1件だけ。**他の履歴テーブルの部分ユニークと
        // 同じ張り方である（24.3）
        manager
            .create_index(
                Index::create()
                    .name("uq_sbom_import_current")
                    .table(SbomImport::Table)
                    .col(SbomImport::DeviceId)
                    .and_where(Expr::col(SbomImport::SupersededAt).is_null())
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SbomComponentChange::Table)
                    .if_not_exists()
                    .col(pk_auto(SbomComponentChange::Id))
                    .col(integer(SbomComponentChange::SbomImportId))
                    // added / removed / version_changed
                    .col(string(SbomComponentChange::ChangeType))
                    .col(string(SbomComponentChange::Name))
                    // 差分計算時の同一性判定キー。無ければ `name` で代替する（9.6）
                    .col(string_null(SbomComponentChange::Purl))
                    .col(string_null(SbomComponentChange::VersionFrom))
                    .col(string_null(SbomComponentChange::VersionTo))
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                SbomComponentChange::Table,
                                SbomComponentChange::SbomImportId,
                            )
                            .to(SbomImport::Table, SbomImport::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_sbom_component_change_import",
            SbomComponentChange::Table,
            SbomComponentChange::SbomImportId,
        )
        .await?;

        // **再構築可能な派生索引。**真実の源は SBOM_SNAPSHOT（9.8）
        manager
            .create_table(
                Table::create()
                    .table(SbomComponentIndex::Table)
                    .if_not_exists()
                    .col(pk_auto(SbomComponentIndex::Id))
                    .col(string(SbomComponentIndex::ContentHash))
                    .col(string_null(SbomComponentIndex::Purl))
                    .col(string(SbomComponentIndex::Name))
                    .col(string(SbomComponentIndex::Version))
                    .foreign_key(
                        ForeignKey::create()
                            .from(SbomComponentIndex::Table, SbomComponentIndex::ContentHash)
                            .to(SbomSnapshot::Table, SbomSnapshot::ContentHash),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_sbom_component_index_content_hash",
            SbomComponentIndex::Table,
            SbomComponentIndex::ContentHash,
        )
        .await?;
        // 「このバージョンのライブラリが入っている機器」を引く（9.8）。
        // `purl` が無いコンポーネントは `name` で引くため、両方に張る
        index(
            manager,
            "idx_sbom_component_index_purl",
            SbomComponentIndex::Table,
            SbomComponentIndex::Purl,
        )
        .await?;
        index(
            manager,
            "idx_sbom_component_index_name",
            SbomComponentIndex::Table,
            SbomComponentIndex::Name,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            SbomComponentIndex::Table.into_iden(),
            SbomComponentChange::Table.into_iden(),
            SbomImport::Table.into_iden(),
            SbomSnapshot::Table.into_iden(),
            SoftwareRoleAssignment::Table.into_iden(),
            SoftwareInstallation::Table.into_iden(),
            SoftwareInstance::Table.into_iden(),
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

// --- 既存のマイグレーションで作成済み。外部キーの参照先としてのみ使う ---

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Device {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum SoftwareCatalog {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum WorkOrder {
    Table,
    Id,
}

// --- 本マイグレーションで作成する ---

/// 列名をそのまま識別子にするため接頭辞が重なる。改名するとDBの列名が変わる。
#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum SoftwareInstance {
    Table,
    Id,
    SoftwareCatalogId,
    LicenseKey,
    AssetNumber,
    RetiredAt,
    CreatedAt,
    UpdatedAt,
}

#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum SoftwareInstallation {
    Table,
    Id,
    SoftwareInstanceId,
    DeviceId,
    WorkOrderId,
    FromDate,
    ToDate,
}

#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum SoftwareRoleAssignment {
    Table,
    Id,
    SoftwareInstallationId,
    Role,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum SbomSnapshot {
    Table,
    ContentHash,
    Content,
    ComponentCount,
    FirstSeenAt,
}

#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum SbomImport {
    Table,
    Id,
    DeviceId,
    ContentHash,
    SourceFormat,
    WorkOrderId,
    ImportedBy,
    ImportedAt,
    SupersededAt,
}

#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum SbomComponentChange {
    Table,
    Id,
    SbomImportId,
    ChangeType,
    Name,
    Purl,
    VersionFrom,
    VersionTo,
}

#[derive(DeriveIden)]
enum SbomComponentIndex {
    Table,
    Id,
    ContentHash,
    Purl,
    Name,
    Version,
}

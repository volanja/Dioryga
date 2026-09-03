//! sea-ormのエンティティ定義。
//!
//! 型の対応は設計書24.2に従う。特に注意すべき点は以下。
//!
//! - **UUIDは文字列で持つ。** PostgreSQLの `uuid` 型を使うとSQLiteと分岐するため。
//!   結合のホットパスに乗らない（主キーは整数の代理キー）ので影響は小さい
//! - **日時は常にUTC。** `DateTimeUtc` を用い、表示時にのみローカル変換する
//! - **JSONは文字列で持つ。** JSON内部を検索しない方針のため `jsonb` の利点がない
//! - **金額は最小通貨単位の整数（i64）で持つ。** SQLiteに `DECIMAL` が無く、
//!   `REAL` では丸め誤差が出るため。本モジュールの対象テーブルには該当列はまだ無い

pub mod app_user;
pub mod audit_log;
pub mod cable_catalog;
pub mod cable_connection;
pub mod cable_end_slot;
pub mod cable_instance;
pub mod chassis_model;
pub mod chassis_slot;
pub mod configuration;
pub mod configuration_part;
pub mod device;
pub mod device_assignment;
pub mod device_mount;
pub mod device_stack;
pub mod firmware_version;
pub mod import_run;
pub mod interface_role;
pub mod interface_stack;
pub mod interface_vlan;
pub mod ip_address;
pub mod login_attempt;
pub mod mount_container;
pub mod os_interface;
pub mod part_catalog;
pub mod part_instance;
pub mod part_instance_location;
pub mod part_port_slot;
pub mod project;
pub mod project_member;
pub mod session;
pub mod software_catalog;
pub mod subnet;
pub mod vendor;
pub mod vlan;
pub mod warehouse;

pub use app_user::Entity as AppUser;
pub use audit_log::Entity as AuditLog;
pub use cable_catalog::Entity as CableCatalog;
pub use cable_connection::Entity as CableConnection;
pub use cable_end_slot::Entity as CableEndSlot;
pub use cable_instance::Entity as CableInstance;
pub use chassis_model::Entity as ChassisModel;
pub use chassis_slot::Entity as ChassisSlot;
pub use configuration::Entity as Configuration;
pub use configuration_part::Entity as ConfigurationPart;
pub use device::Entity as Device;
pub use device_assignment::Entity as DeviceAssignment;
pub use device_mount::Entity as DeviceMount;
pub use device_stack::Entity as DeviceStack;
pub use firmware_version::Entity as FirmwareVersion;
pub use import_run::Entity as ImportRun;
pub use interface_role::Entity as InterfaceRole;
pub use interface_stack::Entity as InterfaceStack;
pub use interface_vlan::Entity as InterfaceVlan;
pub use ip_address::Entity as IpAddress;
pub use login_attempt::Entity as LoginAttempt;
pub use mount_container::Entity as MountContainer;
pub use os_interface::Entity as OsInterface;
pub use part_catalog::Entity as PartCatalog;
pub use part_instance::Entity as PartInstance;
pub use part_instance_location::Entity as PartInstanceLocation;
pub use part_port_slot::Entity as PartPortSlot;
pub use project::Entity as Project;
pub use project_member::Entity as ProjectMember;
pub use session::Entity as Session;
pub use software_catalog::Entity as SoftwareCatalog;
pub use subnet::Entity as Subnet;
pub use vendor::Entity as Vendor;
pub use vlan::Entity as Vlan;
pub use warehouse::Entity as Warehouse;

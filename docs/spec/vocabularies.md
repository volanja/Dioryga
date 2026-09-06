# 語彙（enum値）一覧

設計書の各章に散在するenum値を集めたもの。括弧内は該当章。

## 原則

- **DBに保存する値は英語のキーのまま**とし、表示時にi18nレイヤーで翻訳する（設計書16.5）。`WORK_ORDER.status="planned"` を日本語で保存しない
- **DB制約（CHECK等）にはしない。**アプリケーションが持つ語彙リストで検証する。新しい値の追加のたびにマイグレーションが必要になる形を避けるため（8.6）
- ただし**自由入力にもしない。**登録UIはリストからの選択とする（表記ゆれの防止。18.3でVENDORマスタを作った理由と同じ）

---

## 権限・認証

| 対象 | 値 |
|---|---|
| `PROJECT_MEMBER.role` | `Administrator` / `Operator` / `Viewer` / `Approver` |
| `PROJECT_MEMBER.admin_rank` | `Primary` / `Secondary`（role=Administratorの時のみ） |
| `USER.locale` | `en` / `ja` |

## 機器

| 対象 | 値 |
|---|---|
| `DEVICE.device_type` | `Physical` / `Virtual` / `Container` / `Logical` |
| `DEVICE.status` | `running` / `broken` / `repair` / `plan` / `building` |
| `PART_INSTANCE.status` | 同上 |
| `CABLE_INSTANCE.status` | `in_stock` / `in_use` / `broken` / `disposed` |
| `PROJECT.closure_reason` | `Completed` / `Cancelled`（`archived_at` がある場合のみ） |

**`device_type` の使い分け**：`Virtual` はハイパーバイザ上で動くもの、`Logical` は複数の物理筐体が1台として振る舞うもの（スタック、HAペア）。`Logical` は `configuration_id` と `serial_number` がnullになる。

**`in_stock`/`disposed` はDEVICE/PART_INSTANCEの`status`には含めない。**ロケーション系テーブル（DEVICE_ASSIGNMENT / PART_INSTANCE_LOCATION）から導出する（旧B-1）。

**`SOFTWARE_INSTANCE` は `status` を持たない。**`retired_at`（nullable datetime）のみ。インストール状態は `SOFTWARE_INSTALLATION` から導出する（9.4.1）。

### device_category（CHASSIS_MODEL、または仮想アプライアンスのDEVICE）

`Server` / `Switch` / `Router` / `Firewall` / `LoadBalancer` / `Vpn` / `MediaConverter` / `Storage` / `Pdu` / `Ups` / `Kvm` / `ConsoleServer` / `Other`

多機能アプライアンス（UTM等）は主たる種別を1つ選ぶ。一覧のグループ化・絞り込み用の粗い分類であり、機能の詳細は別の場所が表現する（L2/L3はSVIの有無、ソフトウェア機能は`SOFTWARE_ROLE_ASSIGNMENT`、ネットワーク上の役割は`INTERFACE_ROLE`）。

## カタログ・物理構造

| 対象 | 値 |
|---|---|
| `CHASSIS_MODEL.mount_form` | `RackU` / `RackSide` / `Surface` |
| `CHASSIS_MODEL.rack_width` | `Full` / `Half`（mount_form=RackUの時のみ） |
| `CHASSIS_SLOT.slot_type` | `CPU_SOCKET` / `DIMM` / `DRIVE_BAY` / `PCIE` / `PSU_BAY` |
| `PART_CATALOG.category` | `CPU` / `Memory` / `NIC` / `Storage` / `PSU` / `PDU` |
| `MOUNT_CONTAINER.container_type` | `Rack` / `Desk` / `Shelving` |
| `DEVICE_MOUNT.horizontal_position` | `Left` / `Right` / `Full`（nullable） |
| `DEVICE_MOUNT.depth_position` | `Front` / `Rear` / `Full`（nullable） |

## ネットワーク

| 対象 | 値 |
|---|---|
| `PART_PORT_SLOT.port_kind` | `Network` / `Power` / `Stack` |
| `CABLE_CATALOG.cable_kind` | `Network` / `Power` / `Stack`（8.7。**画面と取込をここで分ける**） |
| `OS_INTERFACE.interface_type` | `Physical` / `Bond` / `Vlan` / `Svi` / `Bridge` / `Virtual` |
| `OS_INTERFACE.aggregation_mode` | `LACP` / `Static` / `ActiveBackup`（interface_type=Bondのみ、nullable） |
| `INTERFACE_VLAN.tagging_mode` | `Untagged` / `Tagged` |

### VLAN.zone / SUBNET.zone（セキュリティ境界）

`DMZ` / `WAN` / `LAN` / `Management` / `Isolated`

**ネットワーク側の属性**。ネットワーク設計者が決める。両方に設定されている場合は `SUBNET.zone` を優先する。

### INTERFACE_ROLE.role（インターフェースの役割）

`Service` / `Management` / `Backup` / `Storage` / `ClusterInterconnect` / `vMotion` ...

**ホスト側の属性**。サーバ運用者が決める。1つのインターフェースに複数付与できる。`zone` とは別軸であり、両者の食い違い（役割がBackupなのにDMZゾーンに載っている等）の検出に用いる。

### 電源（12.7、12.8）

| 対象 | 値 |
|---|---|
| `PORT_POWER_RATING.current_type` | `AC` / `DC` |
| `CONFIGURATION.current_type` | `AC` / `DC` |

**閉じた語彙である。**1つのポートが交流と直流の双方を受けることがあるため、`PORT_POWER_RATING` は方式ごとに1行を持つ。**DCの電圧は負値をとる**（Ciscoの`-48V`電源は`-72`〜`-40`）。符号を含めたまま格納し、絶対値で比較しない。

### connector_type

`CABLE_END_SLOT.connector_type` と `PART_PORT_SLOT.connector_type` で共通の語彙を使う。

- ネットワーク：`RJ-45` / `LC` / `SC` / `MPO12` / `MPO16` / `QSFP28` / `QSFP-DD` / `SFP28ケージ` ...
- 電源：`IEC C13` / `IEC C14` / `IEC C19` / `IEC C20` / `NEMA 5-15P` / `NEMA 5-15R` ...

**互換性判定は一致比較ではなく「対になるか」**（C13⇔C14、NEMA 5-15P⇔5-15R）。適合表が必要（validation.md参照）。

**これは開いた語彙である**（設計書8.6）。上の列挙は代表例であって網羅ではない。**閉じると「表に無いから取り込めない」が常態になる**ため、正規化した自由入力として受け、語彙外でも拒否しない。`port_speed` と `cable_type` も同じ扱いである。

### cable_type

`Cat6` / `Cat6A` / `OM3` / `OM4` / `OM5` / `OS2` / `DAC` / `AOC` / `Power Cord` / `StackWise-480` ...

ファイバのモード（マルチモード=OM3/OM4/OM5、シングルモード=OS2）はここが持つ。コネクタ形状が合っていてもモードが違えば通信できない。

## ソフトウェア

| 対象 | 値 |
|---|---|
| `SOFTWARE_CATALOG.category` | `OS` / `Application` / `Library` / `Framework` |
| `SOFTWARE_ROLE_ASSIGNMENT.role` | `OS` / `DNS` / `NTP` / `WebServer` / `App` ...（運用者が設定、複数可） |
| `SBOM_IMPORT.source_format` | `CycloneDX` / `SPDX` |
| `SBOM_COMPONENT_CHANGE.change_type` | `added` / `removed` / `version_changed` |

## 変更管理

| 対象 | 値 |
|---|---|
| `WORK_ORDER.work_type` | `Repair` / `Addition` / `Relocation` / `Disposal` / `Transfer` |
| `WORK_ORDER.status` | `planned` / `approved` / `executing` / `completed` / `aborted` |
| `WORK_ORDER_APPROVAL.status` | `pending` / `approved` / `rejected` |
| `MILESTONE.milestone_type` | `ServiceStart` / `ServiceUpdate` / `ServiceMaintenance` / `ServiceEnd` |
| `MILESTONE.status` | `planned` / `completed` / `cancelled` |
| `MILESTONE_DEVICE.change_type` | `addition` / `relocation` / `removal` |

### WORK_ORDERの状態遷移

```
[*] --> Planned
Planned  --> Approved   : 承認（Approver）
Approved --> Executing  : 実行開始
Executing --> Completed : 完了
Planned / Approved / Executing --> Aborted : 中止
```

`Aborted` は計画段階・承認後・実行中のいずれからも遷移できる。

## 多態的参照の型

| 対象 | 値 |
|---|---|
| `DEVICE_ASSIGNMENT.location_type` | `Warehouse` / `Project` / `Disposed` |
| `PART_INSTANCE_LOCATION.location_type` | `Warehouse` / `Device` / `Disposed` |
| `MOUNT_CONTAINER.location_type` | `Warehouse` / `Project` |
| `FIRMWARE_VERSION.item_type` | `Device` / `PartInstance` |
| `PURCHASE_ORDER_ITEM.item_type` | `Device` / `PartInstance` / `SoftwareInstance` |
| `FIXED_ASSET.item_type` | 同上 |
| `MAINTENANCE_CONTRACT_ITEM.item_type` | 同上 |
| `RECURRING_COST.item_type` | `MountContainer` / `Project` |

## コスト

| 対象 | 値 |
|---|---|
| `FIXED_ASSET.depreciation_method` | `straight_line` / `declining_balance` |
| `RECURRING_COST.billing_cycle` | `Monthly` / `Annual` |

**定率法（declining_balance）の計算式は未設計。**耐用年数省令の償却率テーブルが必要なため、枠組みだけ用意して先送りしている（設計書10.3、17.1）。

## 取込

| 対象 | 値 |
|---|---|
| `IMPORT_RUN.kind` | `catalog` / `instances` |

## 機器認証情報 — **v2以降**

| 対象 | 値 |
|---|---|
| `DEVICE_CREDENTIAL.purpose` | `BMC` / `SSH` / `SwitchEnable` / `SNMP` ... |
| `DEVICE_CREDENTIAL.storage_type` | `ExternalRef` / `Encrypted` |
| `ENCRYPTED_SECRET.alg` | `aes-256-gcm` / `xchacha20-poly1305` |
| `KEK_GENERATION.custody` | `OsKeystore` / `Passphrase` / `UserWrapped` |
| `KEK_GENERATION.reason` | `Scheduled` / `MemberRevocation` / `AlgMigration` / `Incident` |

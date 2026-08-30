# スキーマ仕様

`../Dioryga_Design/docs/design/basic-design.md` の各章に散在するER図を、実装用に統合したもの。括弧内は設計書の該当章。

**このファイルは暫定の真実の源である。**マイグレーションが動き出した後は、tblsが実DBから生成する出力が正となる（設計書2章）。

## 共通規約

- **主キー**：全テーブルに整数の代理キー `id` を持つ（記載を省略）。`DEVICE.uid` のようなUUIDは別途UNIQUE列として持つ
- **`created_at`/`updated_at`**：履歴テーブル（`from_date`/`to_date` を持つもの）を除く全テーブルに付与する（設計書4章）
- **履歴テーブル**：`to_date IS NULL` が現在有効な行。既存行を更新せず、閉じて新しい行を開く
- **`work_order_id`**：nullable FK。その変更を引き起こしたWORK_ORDER（11章）
- **多態的参照**：`item_type`/`item_id`、`location_type`/`location_id` はDB外部キー制約を持てない。参照先の存在チェックはアプリケーション層で行う（4章、C-7）

---

## 1. 基盤（ユーザー・プロジェクト・認証）

### PROJECT (5章)
| カラム | 型 | 備考 |
|---|---|---|
| uid | uuid | UNIQUE、登録時に採番する不変の識別子 |
| code | string | nullable、UNIQUE。組織のプロジェクトコード |
| name | string | 必須。**一意制約は張らない**（年度違いで同名の案件がありうる） |
| description | string | |
| currency | string | このプロジェクトの集計通貨 |
| archived_at | datetime | nullable、null=進行中。**物理削除しない** |
| closure_reason | string | nullable、`Completed`/`Cancelled`。archived_atがある場合のみ |

**日付とサービス上の状態は持たない。**サービス開始日・終了日は `MILESTONE` の `ServiceStart`/`ServiceEnd` で表現し（10.4）、進行中/完了はそこから導出する。`created_by` も持たない（`AUDIT_LOG` で追跡）。

`archived_at` は**運用者が一覧から外す判断**であり、サービス上の終了とは別概念（5.2）。`closure_reason` は納期遵守率の集計で中止案件と完了案件を区別するために持つ。

取込マニフェストからの解決順序は `uid` → `code` → `name`。`name` で複数該当する場合はエラー（5.3）。

### USER (5章, 20章)
| カラム | 型 | 備考 |
|---|---|---|
| name | string | |
| email | string | UNIQUE、ログインIDを兼ねる |
| password_hash | string | Argon2idのPHC文字列 |
| must_change_password | boolean | 初期パスワード・リセット直後はtrue |
| is_system_admin | boolean | プロジェクトロールとは別軸 |
| locale | string | `en`/`ja` |
| last_login_at | datetime | nullable |
| disabled_at | datetime | nullable、null=有効。**物理削除しない** |

### PROJECT_MEMBER (5章)
| カラム | 型 | 備考 |
|---|---|---|
| user_id | FK | |
| project_id | FK | |
| role | string | Administrator/Operator/Viewer/Approver |
| admin_rank | string | nullable、Primary/Secondary。role=Administratorの時のみ意味を持つ |

粒度は `(user_id, project_id, role)`。**`(user_id, project_id)` に一意制約は張らない**（複数ロール兼務を許すため）。一意制約は `(user_id, project_id, role)` に張る。

### SESSION (20.4)
| カラム | 型 | 備考 |
|---|---|---|
| user_id | FK | |
| token_hash | string | セッショントークンのSHA-256。生の値は保存しない |
| created_at | datetime | |
| last_seen_at | datetime | アイドルタイムアウト判定用 |
| expires_at | datetime | 絶対期限 |
| revoked_at | datetime | nullable |
| ip_address | string | 監査用 |
| user_agent | string | 監査用 |

### API_TOKEN (20.4, 20.9) — **v2**
| カラム | 型 | 備考 |
|---|---|---|
| user_id | FK | サービスアカウントとして扱うUser |
| name | string | 用途がわかるラベル |
| token_hash | string | SHA-256。生の値は発行時に一度だけ表示 |
| expires_at / last_used_at / revoked_at | datetime | nullable |

### LOGIN_ATTEMPT (20.4)
| カラム | 型 | 備考 |
|---|---|---|
| email | string | **FKにしない**（存在しないユーザーへの試行も記録するため） |
| ip_address | string | |
| succeeded | boolean | |
| attempted_at | datetime | |

---

## 2. カタログ（プロジェクト横断の共有マスタ、18章）

### VENDOR (18.3)
`name`, `created_by`(FK User), `created_at`

### CHASSIS_MODEL (6.2, 8.6)
| カラム | 型 | 備考 |
|---|---|---|
| vendor_id | FK | |
| model_name | string | |
| device_category | string | Server/Switch/... 語彙は vocabularies.md |
| height_u | int | ラック搭載時の高さ。mount_form=Surfaceは無視 |
| mount_form | string | RackU/RackSide/Surface |
| rack_width | string | Full/Half、mount_form=RackUの時のみ意味を持つ |
| created_by | FK | User |

自然キー：`(vendor_id, model_name)`

### CHASSIS_SLOT (6.2)
`chassis_model_id`(FK), `slot_type`(CPU_SOCKET/DIMM/DRIVE_BAY/PCIE/PSU_BAY), `slot_label`

**無条件のスロット一覧である。**「4CPU構成でなければ使えない」等の条件付き制約は表現しない（対応しない、と決着済み。設計書6.1/22.2）。

### CONFIGURATION (6.2)
`chassis_model_id`(FK), `name`, `created_by`(FK User)

### CONFIGURATION_PART (6.2)
`configuration_id`(FK), `part_catalog_id`(FK), `quantity`

### PART_CATALOG (6.2, 6.4)
| カラム | 型 | 備考 |
|---|---|---|
| category | string | CPU/Memory/NIC/Storage/PSU/PDU |
| vendor_id | FK | |
| part_number | string | |
| core_count | int | 集計に使うため実カラム化（ハイブリッド方針） |
| capacity_gb | int | 同上 |
| spec_json | json | 周波数・ECC有無・RPM等、集計しない情報 |
| created_by | FK | User |

**`UNIQUE(vendor_id, part_number)`**（7.2ケースE）

### PART_PORT_SLOT (8.3, 8.7)
`part_catalog_id`(FK), `port_kind`(Network/Power/Stack), `port_label`, `connector_type`, `port_speed`(Networkのみ)

### CABLE_CATALOG (8.3)
`cable_type`, `length_m`(decimal), `color`, `vendor_id`(FK), `part_number`, `created_by`(FK User)

### CABLE_END_SLOT (8.3, 8.7)
`cable_catalog_id`(FK), `end_label`(A/B または Trunk/Branch1..N), `connector_type`, `port_speed`

**端ごとに異なる`connector_type`を持てる**（ブレイクアウトケーブル、電源コード、LC-SC変換パッチ）。

### SOFTWARE_CATALOG (9.4)
| カラム | 型 | 備考 |
|---|---|---|
| name | string | |
| vendor_id | FK | |
| version | string | バージョンごとに別レコード |
| category | string | OS/Application/Library/Framework |
| purl | string | nullable |
| license_expression | string | |
| spec_json | json | |
| created_by | FK | User |

`purl` があれば `UNIQUE(purl)`、なければ `UNIQUE(name, vendor_id, version)`。
**SBOM取込はこのテーブルを自動生成しない**（人が資産として登録したものだけ。9.2）。

---

## 3. ハードウェア資産

### DEVICE (6.2, 8.6, 13.2, 23.2, 23.9)
| カラム | 型 | 備考 |
|---|---|---|
| uid | uuid | **UNIQUE**、登録時に採番する不変の識別子 |
| external_id | string | nullable、取込元システムでの識別子（再取込時の突合用） |
| merged_into_device_id | FK(self) | nullable、重複統合で吸収された場合の統合先 |
| merged_at | datetime | nullable |
| configuration_id | FK | nullable、Virtual/Containerは無し |
| device_type | string | Physical/Virtual/Container/**Logical** |
| device_category | string | nullable、`configuration_id IS NULL` の仮想アプライアンス用 |
| hostname | string | 管理名 |
| serial_number | string | nullable、Virtual/Containerは無し |
| asset_number | string | |
| power_watt | int | |
| status | string | running/broken/repair/plan/building |

**`in_stock`/`disposed` は `status` に持たない**（DEVICE_ASSIGNMENTから導出。旧B-1）。

### DEVICE_ASSIGNMENT (6.2) — 履歴
`device_id`(FK), `location_type`(Warehouse/Project/Disposed), `location_id`(nullable、Disposedはnull), `work_order_id`, `from_date`, `to_date`

### PART_INSTANCE (6.2)
`part_catalog_id`(FK), `serial_number`, `status`

### PART_INSTANCE_LOCATION (6.2, 12.4) — 履歴
`part_instance_id`(FK), `location_type`(Warehouse/Device/Disposed), `location_id`, `chassis_slot_id`(nullable、location_type=Deviceのみ、**任意項目**), `work_order_id`, `from_date`, `to_date`

### FIRMWARE_VERSION (6.2) — 履歴
`item_type`(Device/PartInstance), `item_id`, `component`(BIOS/BMC/NIC/RAIDController等), `version`, `work_order_id`, `changed_by`(FK User), `from_date`, `to_date`

### DEVICE_STACK (8.6) — 履歴
`logical_device_id`(FK DEVICE), `member_device_id`(FK DEVICE), `member_number`, `from_date`, `to_date`

スタック全体を `device_type="Logical"` のDEVICEとして登録し、物理筐体は `Physical` のDEVICEとして別に登録する。

| 項目 | 論理Device（Logical） | 物理メンバー（Physical） |
|---|---|---|
| configuration_id / serial_number | null | あり |
| device_category | DEVICE側（`configuration_id IS NULL` のため） | CHASSIS_MODEL側 |
| hostname / 管理IP / OS_INTERFACE / SOFTWARE_INSTALLATION / SBOM | **こちら** | 持たない |
| DEVICE_MOUNT / power_watt / 保守契約 / 固定資産 / 故障履歴 | 持たない（power_wattは0） | **こちら** |
| DEVICE_ASSIGNMENT | **こちら**（RBAC可視性判定のため） | **こちら** |

`Logical` はファイアウォールのHAペア等にも再利用できる。`Virtual` がハイパーバイザ上で動くものを指すのに対し、`Logical` は複数の物理筐体が1台として振る舞うものを指す。

---

## 4. 物理設置・電源（12章）

### WAREHOUSE
`name`, `address`, `created_by`(FK User)

### MOUNT_CONTAINER
`name`, `container_type`(Rack/Desk/Shelving), `location_type`(Warehouse/Project), `location_id`, `capacity`(nullable), `created_by`(FK User)

### DEVICE_MOUNT (12.2, 13.2) — 履歴
| カラム | 備考 |
|---|---|
| device_id | FK |
| container_id | nullable、MOUNT_CONTAINERに直接搭載する場合 |
| position | nullable、Rackなら開始U番号、Shelvingなら段番号 |
| horizontal_position | Left/Right/Full、nullable |
| depth_position | Front/Rear/Full、nullable |
| host_device_id | nullable、他のDeviceの上に載る場合（棚板、VMの実行ホスト） |
| work_order_id | nullable |
| from_date / to_date | |

**1行で `container_id` と `host_device_id` のどちらか一方だけが埋まる。**

---

## 5. ネットワーク（8章）

### OS_INTERFACE (8.3, 8.5) — 履歴
| カラム | 型 | 備考 |
|---|---|---|
| device_id | FK | **必須**（物理ポートを持たないIFがあり導出できないため） |
| interface_type | string | Physical/Bond/Vlan/Svi/Bridge/Virtual |
| part_instance_id | FK | nullable、interface_type=Physicalのみ |
| port_slot_id | FK | nullable、interface_type=Physicalのみ |
| os_interface_name | string | ens1f0, bond0, bond0.100, Eth1/1/1, Vlan100 |
| aggregation_mode | string | nullable、LACP/Static/ActiveBackup。interface_type=Bondのみ |
| work_order_id | FK | nullable |
| from_date / to_date | datetime | |

### INTERFACE_STACK (8.5) — 履歴
`upper_interface_id`(FK), `lower_interface_id`(FK), `from_date`, `to_date`

ボンド（1 upper : N lower）とVLANサブインターフェース（1 lower : N upper）の双方を扱うため中間テーブルとする。

### INTERFACE_VLAN (8.3, 8.6) — 履歴
`os_interface_id`(FK), `vlan_id`(FK), `tagging_mode`(Untagged/Tagged), `work_order_id`, `from_date`, `to_date`

### INTERFACE_ROLE (8.3, 8.5) — 履歴
`os_interface_id`(FK), `role`, `from_date`, `to_date`

### IP_ADDRESS (8.3, 8.6) — 履歴
`os_interface_id`(FK), `ip_address`, `prefix_length`, `subnet_id`(FK nullable), `work_order_id`, `from_date`, `to_date`

**`vlan_id` は持たない**（インターフェース側から辿る。8.6で廃止）。

### VLAN (8.3)
`vlan_tag`, `name`, `zone`(DMZ/WAN/LAN/Management/Isolated), `description`, `created_by`(FK User)

### SUBNET (14.1)
`project_id`(FK nullable), `vlan_id`(FK nullable), `cidr`, `zone`(nullable), `description`, `created_by`(FK User)

**`UNIQUE(subnet_id, ip_address) WHERE to_date IS NULL`** の部分インデックスはPostgreSQL/SQLite双方が対応するため、**例外的にDB制約として実装できる**（14.2）。

### CABLE_INSTANCE (8.3)
`cable_catalog_id`(FK), `serial_number`(nullable), `asset_number`(nullable), `status`(in_stock/in_use/broken/disposed)

### CABLE_CONNECTION (8.3) — 履歴
`cable_instance_id`(FK), `cable_end_slot_id`(FK), `part_instance_id`(FK), `port_slot_id`(FK), `work_order_id`, `from_date`, `to_date`

**`device_id` を持たない**（PART_INSTANCE_LOCATION経由で導出）。

---

## 6. ソフトウェア（9章）

### SOFTWARE_INSTANCE (9.4, 9.4.1)
`software_catalog_id`(FK), `license_key`(nullable), `asset_number`(nullable), `retired_at`(datetime nullable、null=保有中)

**`status` を持たない。**「インストールされているか」は `SOFTWARE_INSTALLATION` の現行行から導出する（旧B-1と同じ判断）。ハードウェアの `broken`/`repair` はライセンスに対応物がなく、稼働中サービスの異常は監視ツールの領域でスコープ外。導出できない「まだ保有しているか」だけを `retired_at` で持つ。

### SOFTWARE_INSTALLATION (9.4) — 履歴
`software_instance_id`(FK), `device_id`(FK), `work_order_id`, `from_date`, `to_date`

### SOFTWARE_ROLE_ASSIGNMENT (9.4, 9.9) — 履歴
`software_installation_id`(FK), `role`, `from_date`, `to_date`

### SBOM_SNAPSHOT (9.5)
`content_hash`(**PK**、正規化後の内容のSHA-256), `content`(blob、zstd圧縮), `component_count`, `first_seen_at`

**内容アドレス指定。**同一内容は1件しか保存しない。

### SBOM_IMPORT (9.5) — 履歴
`device_id`(FK), `content_hash`(FK), `source_format`(CycloneDX/SPDX), `work_order_id`, `imported_by`(FK User), `imported_at`, `superseded_at`(null=最新)

### SBOM_COMPONENT_CHANGE (9.5)
`sbom_import_id`(FK), `change_type`(added/removed/version_changed), `name`, `purl`(nullable), `version_from`(nullable), `version_to`(nullable)

### SBOM_COMPONENT_INDEX (9.5, 9.8)
`content_hash`(FK), `purl`(nullable), `name`, `version`

**再構築可能な派生索引。**真実の源はSBOM_SNAPSHOT。`content_hash`単位に張る（Device単位にしない）。

---

## 7. QCD（10章）

### PURCHASE_ORDER
`order_number`, `order_date`(date), `vendor_id`(FK), `currency`

**`amount` を持たない**（明細合計から計算。旧B-5）。

### PURCHASE_ORDER_ITEM
`purchase_order_id`(FK), `item_type`(Device/PartInstance/SoftwareInstance), `item_id`, `quantity`, `unit_price`(decimal)

### FIXED_ASSET
`item_type`, `item_id`, `acquisition_cost`(decimal), `depreciation_method`(straight_line/declining_balance), `useful_life_years`, `acquisition_date`(date)

**`disposal_date` を持たない**（DEVICE_ASSIGNMENT等の`Disposed`行から求める。旧B-1）。簿価も保存しない。

### MAINTENANCE_CONTRACT
`contract_number`, `vendor_id`(FK), `start_date`, `end_date`, `amount`(decimal), `quote_contact`, `failure_contact`, `purchase_order_id`(FK nullable)

**保守期限はこの `end_date` のみが正**（旧B-2）。

### MAINTENANCE_CONTRACT_ITEM
`maintenance_contract_id`(FK), `item_type`, `item_id`

### RECURRING_COST
`item_type`(MountContainer/Project), `item_id`, `cost_type`, `vendor_id`(FK nullable), `amount`(decimal), `billing_cycle`(Monthly/Annual), `start_date`, `end_date`(nullable), `created_by`(FK User)

### MILESTONE (10.4)
`project_id`(FK), `milestone_type`, `planned_date`(**date**), `actual_date`(date nullable), `status`(planned/completed/cancelled), `description`

**日時粒度は意図的に`date`のまま**（他の履歴テーブルを`datetime`化した理由が当てはまらない。C-5）。

### MILESTONE_DEVICE (10.4)
`milestone_id`(FK), `device_id`(FK), `change_type`(addition/relocation/removal), `work_order_id`(nullable)

---

## 8. 変更管理（11章）

### WORK_ORDER
| カラム | 備考 |
|---|---|
| project_id | 起票元プロジェクト |
| target_project_id | nullable、work_type=Transferの移譲先 |
| device_id | nullable |
| part_instance_id | nullable |
| work_type | Repair/Addition/Relocation/Disposal/Transfer |
| title / description | |
| primary_assignee_id / secondary_assignee_id | FK User、nullable |
| due_date | date nullable |
| status | planned/approved/executing/completed/aborted |
| planned_at / executed_at / completed_at / aborted_at | datetime |
| aborted_reason | nullable |

### WORK_ORDER_APPROVAL
`work_order_id`(FK), `required_project_id`(FK), `approver_id`(FK User nullable), `status`(pending/approved/rejected), `approved_at`(nullable)

WORK_ORDERの`status`は、紐づく**全**WORK_ORDER_APPROVALが`approved`になった時点で`approved`へ遷移する。

**`work_order_id` を持つ履歴テーブル**：PART_INSTANCE_LOCATION, DEVICE_ASSIGNMENT, SOFTWARE_INSTALLATION, FIRMWARE_VERSION, SBOM_IMPORT, CABLE_CONNECTION, DEVICE_MOUNT, OS_INTERFACE, INTERFACE_VLAN, IP_ADDRESS, MILESTONE_DEVICE

---

## 9. 監査・取込

### AUDIT_LOG (15.1)
`user_id`(FK), `table_name`, `record_id`, `action`(insert/update/delete), `before_json`(nullable), `after_json`(nullable), `import_run_id`(FK nullable), `changed_at`

**DBトリガーではなくアプリケーション層（リポジトリ層）で、本来の変更と同一トランザクション内に書き込む。**

### IMPORT_RUN (23.7)
`project_id`(FK nullable、カタログ取込はnull), `kind`(catalog/instances), `file_hash`, `as_of`, `created_count`, `updated_count`, `warning_count`, `imported_by`(FK User), `imported_at`

---

## 10. 機器認証情報 — **v2以降（21章）**

v1では実装しない。テーブル定義のみ記載する。

### DEVICE_CREDENTIAL
`device_id`(FK), `purpose`, `username`(平文), `storage_type`(ExternalRef/Encrypted), `external_ref`(nullable), `description`, `rotation_required_since`(nullable), `created_by`(FK User)

### ENCRYPTED_SECRET
`credential_id`(FK), `alg`, `nonce`(blob), `ciphertext`(blob), `wrapped_dek`(blob), `kek_generation`

### CREDENTIAL_ROTATION
`credential_id`(FK), `rotated_by`(FK User), `rotated_at`, `note`

**旧パスワードの暗号文は保持しない**（4章の履歴原則に対する意識的な例外。21.7）。

### USER_KEY
`user_id`(FK), `public_key`(blob), `wrapped_master_key`(blob), `activated_by`(FK nullable), `activated_at`(nullable), `revoked_at`(nullable)

### KEK_GENERATION
`generation`, `custody`(OsKeystore/Passphrase/UserWrapped), `alg`, `reason`, `created_at`, `activated_at`, `retired_at`(nullable)

//! `CABLE_CATALOG` に種別と定格を足す（設計書8.7）。
//!
//! # 種別（`cable_kind`）
//!
//! ネットワークケーブルと電源ケーブルを**テーブルではなく種別と画面で分ける**
//! （8.7）。配線の実体は `CABLE_INSTANCE` → `CABLE_END_SLOT` → `PART_PORT_SLOT`
//! という接続グラフであり、**流れているのが電気か光かでグラフの形は変わらない。**
//! 分けると `CABLE_CONNECTION`（履歴）が2組になり、接続を辿るクエリがすべて
//! UNIONになる。
//!
//! **`cable_type` からは導かない。**`Power Cord` や `OM4` から種別は導けるように
//! 見えるが、**`cable_type` は開いた語彙である**（8.6）。導出の対応表は作った
//! 時点で不完全であり、新しい `cable_type` が入るたびに種別が決まらなくなる。
//!
//! # なぜ nullable なのか
//!
//! **`cable_kind` は本来必須である。**それでもDB上をnullableにするのは、
//! SQLiteが `NOT NULL` の列を後から足す際にDEFAULTを要求し、**そのDEFAULTが
//! 以後も残る**ためである。残ったDEFAULTは種別未指定のケーブルを黙って
//! `Network` に分類することになり、8.6の「既定値へ倒さず拒否する」に反する。
//!
//! **必須はアプリケーション層で守る。**DBで表現できない制約をアプリ層に置くのは
//! `work_order_id` や多態的参照と同じ扱いである（24.3）。
//!
//! # 定格（8.7、Q-30）
//!
//! **形状が嵌合しても定格が足りなければ使えない。**同じC13でも10A品と15A品があり、
//! コネクタ形状の一致だけでは接続の妥当性を判定できない。適合表（Q-8）は形状の
//! 対応関係を扱い、定格はこの2列が扱う——役割を分ける。
//!
//! **定格は型番から一意に決まる。**12.8で機器の消費電力をカタログに持たせないと
//! 決めたのとは事情が異なり、ケーブルの定格は負荷にも設置環境にも依存しない。
//!
//! 電流は `mA` の整数で持つ（24.2.1）。`length_mm` と同じ理由で、単位を列名に
//! 含めて1000倍の取り違えを防ぐ。
//!
//! # v1で1行も入らないのに、なぜ今入れるか
//!
//! ケーブルの画面・取込はv2（23.8）だが、**`CABLE_CATALOG` は既に存在する
//! テーブルである。**12.7が置いた「v1で使うテーブルへの列追加はv1で入れる／
//! v1で1行も入らない新規テーブルはv2でよい」の前者に近く、後から `ALTER` する
//! より安い。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // **1文につき1列。**SQLiteは `ALTER TABLE` に複数の操作を並べられない
        manager
            .alter_table(
                Table::alter()
                    .table(CableCatalog::Table)
                    .add_column(string_null(CableCatalog::CableKind))
                    .to_owned(),
            )
            .await?;

        for column in [CableCatalog::RatedVoltage, CableCatalog::RatedCurrentMa] {
            manager
                .alter_table(
                    Table::alter()
                        .table(CableCatalog::Table)
                        .add_column(integer_null(column))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            CableCatalog::CableKind,
            CableCatalog::RatedVoltage,
            CableCatalog::RatedCurrentMa,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(CableCatalog::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum CableCatalog {
    Table,
    CableKind,
    RatedVoltage,
    RatedCurrentMa,
}

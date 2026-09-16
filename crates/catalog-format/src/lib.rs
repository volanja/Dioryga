//! Dioryga のカタログ取込フォーマット（設計書23.3、23.4）。
//!
//! # なぜ独立したcrateなのか
//!
//! **構成図パーサ（Krounos）が同じ型を使うため**である。Krounosはこの形式の
//! YAMLを出力する。型を共有しないと展開記法の実装が2つに割れ、**そこが食い違うと
//! 「ドライランは通ったのに本取込でラベルがずれる」という気付きにくい事故**に
//! なる。設計リポジトリ `external-tools.md` 4.2 がKrounosの言語をRustにした
//! 理由としてこの点を挙げている。
//!
//! # 何を置き、何を置かないか
//!
//! **判断の基準は「DBに触れるか」である。**
//!
//! | ここに置く | Dioryga本体に残す |
//! |---|---|
//! | 入力型・展開記法・語彙の検証・`parse` | `dry_run` / `apply`（DBに触れる） |
//! | 形式の誤り（[`FormatError`]） | 差分レポート、`IMPORT_RUN` |
//!
//! **`sea-orm` にも `entity` にも依存しない。**Krounosに ORM を持ち込まない
//! ことが、このcrateを切り出した目的である。
//!
//! # 展開記法（23.4）
//!
//! 64個のDIMMスロットを手書きさせない。`count` ＋ `label_format` で機械的な
//! 連番、`labels` で明示列挙とする。
//!
//! ```yaml
//! slots:
//!   - { slot_type: DIMM, count: 64, label_format: "DIMM{n}" }
//!   - { slot_type: PCIE, labels: [Slot1, Slot2] }
//! ```
//!
//! YAMLのアンカー（`&dimm64` / `*dimm64`）はパーサが解決するため、こちらで
//! 扱う必要はない。
//!
//! # 閉じた語彙と開いた語彙（8.6）
//!
//! 拒否するのは**閉じた語彙**——`device_category`・`port_kind`・`current_type`——だけである。
//! `connector_type` と `port_speed` は `vocabularies.md` でも末尾が `...` の
//! 開いた列挙であり、**閉じると「表に無いから取り込めない」が常態になる。**
//! コネクタ形状も速度表記もベンダーと世代で増え続けるためで、正規化した
//! 自由入力として受ける。

use serde::Deserialize;

/// 対応するフォーマットの版。
pub const FORMAT_VERSION: u32 = 1;
pub const KIND: &str = "catalog";

/// 形式の誤り。**DB由来の失敗はここに入れない**（本体の `ImportError` が持つ）。
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("YAMLの形式が正しくありません: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),

    #[error("format_version が {found} です。対応しているのは {expected} です")]
    UnsupportedVersion { found: u32, expected: u32 },

    #[error("kind が {found} です。このコマンドが扱うのは {expected} です")]
    UnexpectedKind { found: String, expected: String },
}

// ---------------------------------------------------------------------------
// ファイルの構造
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CatalogFile {
    pub format_version: u32,
    pub kind: String,
    #[serde(default)]
    pub vendors: Vec<VendorInput>,
    #[serde(default)]
    pub chassis_models: Vec<ChassisModelInput>,
    #[serde(default)]
    pub part_catalogs: Vec<PartCatalogInput>,
    #[serde(default)]
    pub configurations: Vec<ConfigurationInput>,
    /// VLAN（8.5）。**プロジェクトを横断するマスタなのでカタログ側に置く**
    /// （23.5）。インスタンスCSVに置くと「1ファイルは1プロジェクトに閉じる」
    /// が崩れ、あるプロジェクトの取込が他プロジェクトの見るVLANを書き換える。
    #[serde(default)]
    pub vlans: Vec<VlanInput>,
}

#[derive(Debug, Deserialize)]
pub struct VlanInput {
    pub vlan_tag: i32,
    pub name: String,
    /// DMZ / WAN / LAN / Management / Isolated。**未設定を許す。**
    #[serde(default)]
    pub zone: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct VendorInput {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct ChassisModelInput {
    pub vendor: String,
    pub model_name: String,
    pub device_category: String,
    #[serde(default)]
    pub height_u: i32,
    pub mount_form: String,
    #[serde(default)]
    pub rack_width: Option<String>,
    #[serde(default)]
    pub slots: Vec<SlotInput>,
}

/// スロットの展開記法（23.4）。
///
/// `count` ＋ `label_format` か、`labels` のいずれかを使う。
#[derive(Debug, Deserialize)]
pub struct SlotInput {
    pub slot_type: String,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub label_format: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct PartCatalogInput {
    pub vendor: String,
    pub part_number: String,
    pub category: String,
    #[serde(default)]
    pub core_count: Option<i32>,
    #[serde(default)]
    pub capacity_gb: Option<i32>,
    #[serde(default)]
    pub spec_json: Option<serde_json::Value>,
    /// ポート定義（8.3）。**スロットと同じ展開記法**（23.4）。
    #[serde(default)]
    pub ports: Vec<PortInput>,
}

/// ポートの展開記法（23.4）。`count` ＋ `label_format` か `labels`。
#[derive(Debug, Deserialize)]
pub struct PortInput {
    /// **閉じた語彙**（8.6）。Network / Power / Stack
    pub port_kind: String,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub label_format: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// **開いた語彙**（8.6）。正規化した自由入力として受ける
    pub connector_type: String,
    /// `port_kind=Network` のときだけ意味を持つ。
    #[serde(default)]
    pub port_speed: Option<String>,
    /// `port_kind=Power` のときだけ意味を持つ（12.7）。
    ///
    /// **展開したポートすべてが同じ定格を持つ**（23.4）。同じ型番のインレットが
    /// 2口あって片方だけ定格が違う、ということは起こらない。
    #[serde(default)]
    pub power_ratings: Vec<PowerRatingInput>,
}

#[derive(Debug, Deserialize)]
pub struct PowerRatingInput {
    /// **閉じた語彙**（8.6）。AC / DC
    pub current_type: String,
    /// **DCは負値をとる。**符号を含めたまま持ち、絶対値で比べない（12.7）。
    pub voltage_min: i32,
    pub voltage_max: i32,
}

#[derive(Debug, Deserialize)]
pub struct ConfigurationInput {
    pub chassis_model: ChassisModelRef,
    pub name: String,
    #[serde(default)]
    pub parts: Vec<ConfigurationPartInput>,
}

#[derive(Debug, Deserialize)]
pub struct ChassisModelRef {
    pub vendor: String,
    pub model_name: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfigurationPartInput {
    /// アンカーで部品定義を丸ごと参照できる（23.4）。自然キーだけを見る。
    pub part: PartCatalogRef,
    #[serde(default = "一つ")]
    pub quantity: i32,
}

fn 一つ() -> i32 {
    1
}

#[derive(Debug, Deserialize)]
pub struct PartCatalogRef {
    pub vendor: String,
    pub part_number: String,
}
// ---------------------------------------------------------------------------
// 展開
// ---------------------------------------------------------------------------

impl SlotInput {
    /// ラベルの一覧へ展開する。
    pub fn expand(&self) -> Result<Vec<String>, String> {
        展開(
            self.labels.as_deref(),
            self.count,
            self.label_format.as_deref(),
            &self.slot_type,
        )
    }
}

impl PortInput {
    /// ラベルの一覧へ展開する。**スロットと同じ記法**（23.4）。
    pub fn expand(&self) -> Result<Vec<String>, String> {
        展開(
            self.labels.as_deref(),
            self.count,
            self.label_format.as_deref(),
            &self.port_kind,
        )
    }
}

/// 展開記法の本体（23.4）。
///
/// `{n}` は1始まりの連番に置き換える。**0始まりにしない**のは、
/// 実機のラベル（DIMM1、Bay1、Port1）が1始まりであるため。
fn 展開(
    labels: Option<&[String]>,
    count: Option<u32>,
    label_format: Option<&str>,
    既定の接頭辞: &str,
) -> Result<Vec<String>, String> {
    match (labels, count) {
        (Some(labels), None) => {
            if labels.is_empty() {
                return Err("labels が空です".to_owned());
            }
            Ok(labels.to_vec())
        }
        (None, Some(count)) => {
            if count == 0 {
                return Err("count が 0 です".to_owned());
            }
            // 実機のスロット数として現実的な範囲を超えたら、記述の誤りを疑う
            if count > 1024 {
                return Err(format!("count が大きすぎます（{count}）"));
            }
            let format = label_format
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{既定の接頭辞}{{n}}"));
            Ok((1..=count)
                .map(|n| format.replace("{n}", &n.to_string()))
                .collect())
        }
        (Some(_), Some(_)) => Err("labels と count は同時に指定できません".to_owned()),
        (None, None) => Err("labels か count のどちらかが要ります".to_owned()),
    }
}
// ---------------------------------------------------------------------------
// ポートの検証（設計書8.3、8.6、12.7）
// ---------------------------------------------------------------------------

/// 閉じた語彙（8.6）。**リスト外は拒否する。**
const PORT_KINDS: &[&str] = &["Network", "Power", "Stack"];
/// セキュリティ境界（8.5）。**閉じた語彙**——`vocabularies.md` の列挙が閉じており、
/// 表に無い値を受けると `dioryga check` の「役割とゾーンの不整合」が判定できない。
const ZONES: &[&str] = &["DMZ", "WAN", "LAN", "Management", "Isolated"];

/// IEEE 802.1Q。**0と4095は予約されている。**
const VLANタグの下限: i32 = 1;
const VLANタグの上限: i32 = 4094;
const CURRENT_TYPES: &[&str] = &["AC", "DC"];

/// 機器の種別（8.6）。**閉じた語彙。**`CHASSIS_MODEL` と、構成を持たない
/// `DEVICE` の両方が使う。画面と取込が同じ表を見るよう、ここにだけ置く。
///
/// **略語は大文字で書く**（`VPN` / `PDU` / `UPS` / `KVM`、#125）。
pub const DEVICE_CATEGORIES: &[&str] = &[
    "Server",
    "Switch",
    "Router",
    "Firewall",
    "LoadBalancer",
    "VPN",
    "MediaConverter",
    "Storage",
    "PDU",
    "UPS",
    "KVM",
    "ConsoleServer",
    "Other",
];

/// 種別を検証する。**大文字・小文字を寄せずに拒否する**（#125）。
///
/// 旧表記の `Vpn` 等を黙って `VPN` に直すと、語彙が経路ごとに2通りになる。
/// 語彙外の値は既定へ倒さず拒否する（Q-21）のと同じ扱いにする。
pub fn 種別を検証する(value: &str) -> Result<(), String> {
    if DEVICE_CATEGORIES.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "device_category「{value}」は語彙にありません（{}）",
            DEVICE_CATEGORIES.join(" / ")
        ))
    }
}

const NETWORK: &str = "Network";
const POWER: &str = "Power";

/// 検証を通ったポート1本ぶん。
pub struct 展開後のポート {
    pub port_kind: String,
    pub port_label: String,
    pub connector_type: String,
    pub port_speed: Option<String>,
    pub ratings: Vec<(String, i32, i32)>,
}

/// ポート定義を検証して展開する。
///
/// **エラーは取り込まない、警告は取り込むが記録する**（23.6）。ここで警告に
/// なるのは「値そのものは読めるが、その `port_kind` では意味を持たない」場合で、
/// **語彙外の値（拒否）とは別の話である。**
pub fn ポートを検証する(
    ports: &[PortInput],
) -> Result<(Vec<展開後のポート>, Vec<String>), String> {
    let mut 出力 = Vec::new();
    let mut 警告 = Vec::new();

    for p in ports {
        // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
        if !PORT_KINDS.contains(&p.port_kind.as_str()) {
            return Err(format!("port_kind「{}」は語彙にありません", p.port_kind));
        }
        let connector = 正規化(&p.connector_type);
        if connector.is_empty() {
            return Err(format!("{}: connector_type が空です", p.port_kind));
        }

        let labels = p.expand().map_err(|e| format!("{}: {e}", p.port_kind))?;

        // **意味を持たない列は捨てるが、黙って捨てない**（23.6、12.7）
        let port_speed = match p.port_speed.as_deref().map(正規化) {
            Some(s) if s.is_empty() => None,
            Some(s) if p.port_kind == NETWORK => Some(s),
            Some(s) => {
                警告.push(format!(
                    "port_speed「{s}」は port_kind={} では意味を持たないため無視しました",
                    p.port_kind
                ));
                None
            }
            None => None,
        };

        let ratings = if p.power_ratings.is_empty() {
            Vec::new()
        } else if p.port_kind != POWER {
            警告.push(format!(
                "power_ratings は port_kind={} では意味を持たないため無視しました",
                p.port_kind
            ));
            Vec::new()
        } else {
            定格を検証する(&p.power_ratings)?
        };

        for label in labels {
            出力.push(展開後のポート {
                port_kind: p.port_kind.clone(),
                port_label: label,
                connector_type: connector.clone(),
                port_speed: port_speed.clone(),
                // **展開したポートすべてが同じ定格を持つ**（23.4）
                ratings: ratings.clone(),
            });
        }
    }

    Ok((出力, 警告))
}

fn 定格を検証する(ratings: &[PowerRatingInput]) -> Result<Vec<(String, i32, i32)>, String> {
    let mut 出力: Vec<(String, i32, i32)> = Vec::new();

    for r in ratings {
        if !CURRENT_TYPES.contains(&r.current_type.as_str()) {
            return Err(format!(
                "current_type「{}」は語彙にありません",
                r.current_type
            ));
        }
        // **絶対値ではなく符号を含めた大小で見る**（12.7）。DCは -72 が下限
        if r.voltage_min > r.voltage_max {
            return Err(format!(
                "{}: 電圧の下限（{}）が上限（{}）を超えています",
                r.current_type, r.voltage_min, r.voltage_max
            ));
        }
        // **方式ごとに1行**（12.7）。DBのUNIQUEに任せず、ここで理由を返す
        if 出力.iter().any(|(k, _, _)| *k == r.current_type) {
            return Err(format!(
                "current_type「{}」が重複しています",
                r.current_type
            ));
        }
        出力.push((r.current_type.clone(), r.voltage_min, r.voltage_max));
    }

    Ok(出力)
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

/// 解析して形式を確かめる。
/// VLANの行を検証し、正規化した `zone` を返す。
///
/// **同じタグが複数あることは禁じない**（8.5）。VLANタグはL2ドメインごとに
/// 独立しており、**拠点が違えば同じ `VLAN 100` が別物として存在する。**
/// 一意にすると複数拠点を1つの台帳で扱えなくなる。取り違えの警告は
/// DBを見る側（本体）が出す。
pub fn vlanを検証する(v: &VlanInput) -> Result<Option<String>, String> {
    if !(VLANタグの下限..=VLANタグの上限).contains(&v.vlan_tag) {
        return Err(format!(
            "vlan_tag「{}」は{VLANタグの下限}〜{VLANタグの上限}の範囲外です",
            v.vlan_tag
        ));
    }
    if 正規化(&v.name).is_empty() {
        return Err("name が空です".to_owned());
    }
    match v.zone.trim() {
        "" => Ok(None),
        z if ZONES.contains(&z) => Ok(Some(z.to_owned())),
        // **閉じた語彙は既定へ寄せず拒否する**（8.6）
        z => Err(format!("zone「{z}」は使えません（{}）", ZONES.join(" / "))),
    }
}

pub fn parse(source: &str) -> Result<CatalogFile, FormatError> {
    let file: CatalogFile = serde_yaml_ng::from_str(source)?;

    if file.format_version != FORMAT_VERSION {
        return Err(FormatError::UnsupportedVersion {
            found: file.format_version,
            expected: FORMAT_VERSION,
        });
    }
    if file.kind != KIND {
        return Err(FormatError::UnexpectedKind {
            found: file.kind.clone(),
            expected: KIND.to_owned(),
        });
    }

    Ok(file)
}

// ---------------------------------------------------------------------------
// 正規化（設計書18.4）
// ---------------------------------------------------------------------------

/// 全角英数を半角に、連続する空白を1つに（18.4、25.2の段階3）。
///
/// **仮名・漢字は触らない。**日本語のベンダー名や説明を壊さないため。
///
/// **開いた語彙への手当てはここが担う**（8.6）。語彙外を拒否するのではなく、
/// 表記を揃えたうえで受け、残った揺れは統合（18.5）で片付ける。
///
/// **Dioryga本体もこの実装を使う。**取込と手入力で正規化が食い違うと、
/// 同じ値が経路によって別行になる。
pub fn 正規化(value: &str) -> String {
    let 半角: String = value
        .chars()
        .map(|c| match c {
            // 全角英数・記号は半角へ。全角空白も通常の空白へ
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect();

    半角.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn スロット(yaml: &str) -> SlotInput {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    fn vlan(yaml: &str) -> VlanInput {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[test]
    fn vlanのタグは802_1qの範囲に限る() {
        // **0と4095は予約されている。**実機に設定できない値を受けない
        assert!(vlanを検証する(&vlan("{ vlan_tag: 0, name: a }")).is_err());
        assert!(vlanを検証する(&vlan("{ vlan_tag: 4095, name: a }")).is_err());
        assert!(vlanを検証する(&vlan("{ vlan_tag: 1, name: a }")).is_ok());
        assert!(vlanを検証する(&vlan("{ vlan_tag: 4094, name: a }")).is_ok());
    }

    #[test]
    fn vlanのゾーンは閉じた語彙() {
        // 未設定は許し、表に無い値は**既定へ寄せず拒否する**（8.6）
        assert_eq!(
            vlanを検証する(&vlan("{ vlan_tag: 100, name: a }")).unwrap(),
            None
        );
        assert_eq!(
            vlanを検証する(&vlan("{ vlan_tag: 100, name: a, zone: DMZ }")).unwrap(),
            Some("DMZ".to_owned())
        );
        assert!(vlanを検証する(&vlan("{ vlan_tag: 100, name: a, zone: まんなか }")).is_err());
    }

    #[test]
    fn vlanの名前は必須() {
        // 同じタグが複数ありうる以上、名前が無いと区別が付かない（8.5）
        assert!(vlanを検証する(&vlan("{ vlan_tag: 100, name: \"  \" }")).is_err());
    }

    #[test]
    fn 連番へ展開される() {
        let s = スロット(r#"{ slot_type: DIMM, count: 4, label_format: "DIMM{n}" }"#);
        assert_eq!(s.expand().unwrap(), ["DIMM1", "DIMM2", "DIMM3", "DIMM4"]);
    }

    /// **1始まりであること。**実機のラベル（DIMM1、Bay1）に合わせる。
    #[test]
    fn 連番は1始まり() {
        let s = スロット(r#"{ slot_type: DIMM, count: 1, label_format: "DIMM{n}" }"#);
        assert_eq!(s.expand().unwrap(), ["DIMM1"]);
    }

    #[test]
    fn ラベルの明示列挙ができる() {
        let s = スロット(r#"{ slot_type: PCIE, labels: [Slot1, Slot9] }"#);
        assert_eq!(s.expand().unwrap(), ["Slot1", "Slot9"]);
    }

    #[test]
    fn label_formatが無ければslot_typeを使う() {
        let s = スロット("{ slot_type: PSU_BAY, count: 2 }");
        assert_eq!(s.expand().unwrap(), ["PSU_BAY1", "PSU_BAY2"]);
    }

    /// 両方指定・どちらも無しは記述の誤りとして弾く。
    #[test]
    fn 曖昧な指定は拒否される() {
        assert!(スロット(r#"{ slot_type: DIMM, count: 2, labels: [A, B] }"#)
            .expand()
            .is_err());
        assert!(スロット("{ slot_type: DIMM }").expand().is_err());
        assert!(スロット("{ slot_type: DIMM, count: 0 }").expand().is_err());
    }

    /// 桁を打ち間違えた場合に、64本のつもりが6400本入るのを防ぐ。
    #[test]
    fn 現実的でない本数は拒否される() {
        assert!(スロット("{ slot_type: DIMM, count: 5000 }")
            .expand()
            .is_err());
    }

    #[test]
    fn 版と種別が違えば読まない() {
        let 別の版 = "format_version: 99\nkind: catalog\n";
        assert!(matches!(
            parse(別の版),
            Err(FormatError::UnsupportedVersion { .. })
        ));

        let 別の種別 = "format_version: 1\nkind: instances\n";
        assert!(matches!(
            parse(別の種別),
            Err(FormatError::UnexpectedKind { .. })
        ));
    }

    /// YAMLのアンカーがパーサ側で解決されること（23.4）。
    #[test]
    fn アンカーで部品を参照できる() {
        let yaml = r#"
format_version: 1
kind: catalog
part_catalogs:
  - &dimm
    vendor: Cisco
    part_number: UCS-MRX64G2RE5
    category: Memory
    capacity_gb: 64
configurations:
  - chassis_model: { vendor: Fujitsu, model_name: RX4770 M8 }
    name: 標準構成
    parts:
      - { part: *dimm, quantity: 16 }
"#;
        let file = parse(yaml).unwrap();
        let part = &file.configurations[0].parts[0];
        assert_eq!(part.part.part_number, "UCS-MRX64G2RE5");
        assert_eq!(part.quantity, 16);
    }

    #[test]
    fn 数量の既定は1() {
        let yaml = r#"
format_version: 1
kind: catalog
configurations:
  - chassis_model: { vendor: V, model_name: M }
    name: c
    parts:
      - { part: { vendor: V, part_number: P } }
"#;
        let file = parse(yaml).unwrap();
        assert_eq!(file.configurations[0].parts[0].quantity, 1);
    }

    /// **ポートもスロットと同じ記法で展開されること**（23.4）。
    #[test]
    fn ポートも同じ記法で展開される() {
        let p: PortInput = serde_yaml_ng::from_str(
            r#"{ port_kind: Network, count: 2, label_format: "Port{n}", connector_type: SFP28 }"#,
        )
        .unwrap();
        assert_eq!(p.expand().unwrap(), ["Port1", "Port2"]);
    }

    /// **閉じた語彙は拒否し、開いた語彙は受けること**（8.6）。
    ///
    /// ここが逆になると、構成図の取込が「表に無いから取り込めない」で止まる。
    #[test]
    fn 閉じた語彙だけを拒否する() {
        let 語彙外: PortInput = serde_yaml_ng::from_str(
            r#"{ port_kind: ネットワーク, count: 1, connector_type: RJ-45 }"#,
        )
        .unwrap();
        assert!(ポートを検証する(&[語彙外]).is_err());

        // connector_type は開いた語彙。表に無くても通る
        let 未知: PortInput = serde_yaml_ng::from_str(
            r#"{ port_kind: Network, count: 1, connector_type: OSFP-XD, port_speed: 1.6T }"#,
        )
        .unwrap();
        let (ports, 警告) = ポートを検証する(&[未知]).unwrap();
        assert_eq!(ports[0].connector_type, "OSFP-XD");
        assert!(警告.is_empty());
    }

    /// **DCの負電圧を絶対値で比べないこと**（12.7）。`-72 ≤ -40`。
    #[test]
    fn 直流の負電圧を受け付ける() {
        let p: PortInput = serde_yaml_ng::from_str(
            r#"{ port_kind: Power, count: 1, connector_type: "IEC C14",
                 power_ratings: [{ current_type: DC, voltage_min: -72, voltage_max: -40 }] }"#,
        )
        .unwrap();
        let (ports, _) = ポートを検証する(&[p]).unwrap();
        assert_eq!(ports[0].ratings[0], ("DC".to_owned(), -72, -40));
    }

    /// **意味を持たない列は捨てるが、黙って捨てない**（23.6）。
    #[test]
    fn 意味を持たない列は警告になる() {
        let p: PortInput = serde_yaml_ng::from_str(
            r#"{ port_kind: Power, count: 1, connector_type: "IEC C14", port_speed: 25G }"#,
        )
        .unwrap();
        let (ports, 警告) = ポートを検証する(&[p]).unwrap();
        assert_eq!(ports.len(), 1, "ポート自体は作られるべき");
        assert!(ports[0].port_speed.is_none());
        assert_eq!(警告.len(), 1);
    }

    /// **略語の旧表記（`Vpn` 等）は受けない**（#125）。
    #[test]
    fn 種別は大文字小文字を区別して検証する() {
        for v in ["VPN", "PDU", "UPS", "KVM", "Server"] {
            assert!(種別を検証する(v).is_ok(), "{v}");
        }
        for v in ["Vpn", "Pdu", "Ups", "Kvm", "server", ""] {
            assert!(種別を検証する(v).is_err(), "{v}");
        }
    }

    /// **正規化は仮名・漢字を壊さない**（18.4）。
    #[test]
    fn 正規化は全角英数だけを直す() {
        assert_eq!(正規化("  ＨＰＥ  "), "HPE");
        assert_eq!(正規化("富士通　の　製品"), "富士通 の 製品");
    }
}

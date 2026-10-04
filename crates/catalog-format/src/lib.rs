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
//! 拒否するのは**閉じた語彙**——`device_category`・`mount_form`・`rack_width`・
//! `slot_type`・部品の `category`・`port_kind`・`current_type`——だけである。
//! `connector_type` と `port_speed` は `vocabularies.md` でも末尾が `...` の
//! 開いた列挙であり、**閉じると「表に無いから取り込めない」が常態になる。**
//! コネクタ形状も速度表記もベンダーと世代で増え続けるためで、正規化した
//! 自由入力として受ける。

use serde::Deserialize;

// ---------------------------------------------------------------------------
// 理由（#214）
// ---------------------------------------------------------------------------

/// 利用者に見せる理由。**文言ではなく、キーと差し込む値で持つ**（#214）。
///
/// 同じ理由を、画面は利用者の言語で、コンソールはOSの言語で出す。理由を作る
/// 時点ではどちらの言語で出すかが決まらないため、**言語は表示する側が渡す。**
/// このcrateは言語に依存しない。キーの文言は Dioryga 本体の locales
/// （`import_detail.*`）にある。
///
/// 差し込む値には、別の理由を入れ子にできる（[`Arg::Message`]）。「`status`: 語彙外」
/// のように、理由に項目名を前置する場合に使う。
///
/// キーが空の `Message`（[`Message::default`]）は「理由なし」を表す。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    pub key: &'static str,
    pub args: Vec<(&'static str, Arg)>,
}

/// [`Message`] に差し込む値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    /// そのまま差し込む値（利用者が書いた値、件数など）。訳さない。
    Text(String),
    /// 入れ子の理由。表示する側が同じ言語で訳す。
    Message(Message),
    /// 入れ子の理由の並び。表示する側が訳し、言語の区切りでつなぐ。
    List(Vec<Message>),
}

impl Message {
    pub fn new(key: &'static str) -> Self {
        Self {
            key,
            args: Vec::new(),
        }
    }

    /// 差し込む値を足す。`name` は文言の `%{name}` に当たる。
    pub fn with(mut self, name: &'static str, value: impl Into<Arg>) -> Self {
        self.args.push((name, value.into()));
        self
    }

    /// 理由が無いか。
    pub fn is_empty(&self) -> bool {
        self.key.is_empty()
    }
}

/// 訳さずにキーと値を並べる。**利用者に見せる文言ではない**（ログ・デバッグ用）。
impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key)?;
        for (name, value) in &self.args {
            match value {
                Arg::Text(v) => write!(f, " {name}={v}")?,
                Arg::Message(m) => write!(f, " {name}=({m})")?,
                Arg::List(ms) => {
                    write!(f, " {name}=[")?;
                    for (i, m) in ms.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "({m})")?;
                    }
                    f.write_str("]")?;
                }
            }
        }
        Ok(())
    }
}

impl From<Message> for Arg {
    fn from(v: Message) -> Self {
        Self::Message(v)
    }
}

impl From<Vec<Message>> for Arg {
    fn from(v: Vec<Message>) -> Self {
        Self::List(v)
    }
}

impl From<String> for Arg {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

impl From<&String> for Arg {
    fn from(v: &String) -> Self {
        Self::Text(v.clone())
    }
}

impl From<&str> for Arg {
    fn from(v: &str) -> Self {
        Self::Text(v.to_owned())
    }
}

/// **移行中だけの口**（#214）。訳されていない文言をそのまま運ぶ。
/// 取込の種類ごとにキーへ移し終えたら消す。
pub const 未訳: &str = "_raw";

impl From<String> for Message {
    fn from(v: String) -> Self {
        if v.is_empty() {
            return Self::default();
        }
        Self::new(未訳).with("text", v)
    }
}

impl From<&str> for Message {
    fn from(v: &str) -> Self {
        Self::from(v.to_owned())
    }
}

macro_rules! 数を差し込む {
    ($($t:ty),*) => {
        $(impl From<$t> for Arg {
            fn from(v: $t) -> Self {
                Self::Text(v.to_string())
            }
        })*
    };
}
数を差し込む!(i32, i64, u32, u64, usize);

/// [`Message`] を組み立てる。`理由!("import_detail.format.count_zero")`、
/// `理由!("import_detail.format.not_in_vocabulary", field = "slot_type", value = v)`。
#[macro_export]
macro_rules! 理由 {
    ($key:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::Message::new($key)$(.with(stringify!($name), $value))*
    };
}

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
    /// 設備・什器の型番（設計書12.10）。ラック・机・棚。
    #[serde(default)]
    pub container_models: Vec<ContainerModelInput>,
}

/// 設備・什器の型番（12.10）。**種別で使う収容能力の列が分かれる**——Rack は
/// `height_u`、Shelving は `shelf_count`、Desk はどちらも持たない。
///
/// 寸法はミリメートル、重量と静荷重はグラムの整数（24.2.1）。
#[derive(Debug, Deserialize)]
pub struct ContainerModelInput {
    pub vendor: String,
    pub model_name: String,
    pub container_type: String,
    #[serde(default)]
    pub height_u: Option<i32>,
    #[serde(default)]
    pub shelf_count: Option<i32>,
    #[serde(default)]
    pub width_mm: Option<i32>,
    #[serde(default)]
    pub depth_mm: Option<i32>,
    #[serde(default)]
    pub height_mm: Option<i32>,
    #[serde(default)]
    pub weight_g: Option<i32>,
    #[serde(default)]
    pub static_load_g: Option<i32>,
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
    /// 重量（g）。設備・什器の静荷重と比べるため（12.10）。
    #[serde(default)]
    pub weight_g: Option<i32>,
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
    /// ラベルの一覧へ展開する。**`slot_type` の語彙もここで確かめる**（8.6、#148）。
    ///
    /// ドライランと反映の両方がここを通るため、検証を外に置くと片方で漏れる。
    pub fn expand(&self) -> Result<Vec<String>, Message> {
        閉じた語彙("slot_type", &self.slot_type, SLOT_TYPES)?;
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
    pub fn expand(&self) -> Result<Vec<String>, Message> {
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
) -> Result<Vec<String>, Message> {
    match (labels, count) {
        (Some(labels), None) => {
            if labels.is_empty() {
                return Err(理由!("import_detail.format.labels_empty"));
            }
            Ok(labels.to_vec())
        }
        (None, Some(count)) => {
            if count == 0 {
                return Err(理由!("import_detail.format.count_zero"));
            }
            // 実機のスロット数として現実的な範囲を超えたら、記述の誤りを疑う
            if count > 1024 {
                return Err(理由!(
                    "import_detail.format.count_too_large",
                    count = count
                ));
            }
            let format = label_format
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{既定の接頭辞}{{n}}"));
            Ok((1..=count)
                .map(|n| format.replace("{n}", &n.to_string()))
                .collect())
        }
        (Some(_), Some(_)) => Err(理由!("import_detail.format.labels_and_count")),
        (None, None) => Err(理由!("import_detail.format.labels_or_count")),
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

/// `CHASSIS_MODEL.mount_form`（6.2、12.3）。**閉じた語彙。**
pub const MOUNT_FORMS: &[&str] = &["RackU", "RackSide", "Surface"];
/// `CHASSIS_MODEL.rack_width`。**`mount_form=RackU` のときだけ意味を持つ。**
pub const RACK_WIDTHS: &[&str] = &["Full", "Half"];
/// `CHASSIS_SLOT.slot_type`。**閉じた語彙。**
pub const SLOT_TYPES: &[&str] = &["CPU_SOCKET", "DIMM", "DRIVE_BAY", "PCIE", "PSU_BAY"];
/// `PART_CATALOG.category`（6.4）。**閉じた語彙。**
pub const PART_CATEGORIES: &[&str] = &["CPU", "Memory", "NIC", "Storage", "PSU", "PDU"];
/// `CONTAINER_MODEL.container_type`（12.10）。**閉じた語彙。**画面と取込が同じ表を見る。
pub const CONTAINER_TYPES: &[&str] = &["Rack", "Desk", "Shelving"];

/// 閉じた語彙で検証する。**大文字・小文字を寄せず、既定へも倒さない**（Q-21）。
fn 閉じた語彙(項目: &str, value: &str, allowed: &[&str]) -> Result<(), Message> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(理由!(
            "import_detail.format.not_in_vocabulary",
            field = 項目,
            value = value,
            allowed = allowed.join(" / ")
        ))
    }
}

/// 搭載形態と幅を検証し、保存する `rack_width` を返す（6.2、#148）。
///
/// **画面と同じ規則にする**（8.6「手入力の画面と取込で扱いを揃える」）。
/// `RackU` で幅が無ければ `Full`、`RackU` 以外で幅があればエラー
/// （意味を持たない値を黙って捨てない）。
pub fn 搭載を検証する(
    mount_form: &str,
    rack_width: Option<&str>,
) -> Result<Option<String>, Message> {
    閉じた語彙("mount_form", mount_form, MOUNT_FORMS)?;
    match (mount_form, rack_width.map(str::trim).unwrap_or("")) {
        ("RackU", "") => Ok(Some("Full".to_owned())),
        ("RackU", w) => 閉じた語彙("rack_width", w, RACK_WIDTHS).map(|_| Some(w.to_owned())),
        (_, "") => Ok(None),
        (m, w) => Err(理由!(
            "import_detail.format.rack_width_only_racku",
            width = w,
            mount_form = m
        )),
    }
}

/// 部品のカテゴリを検証する（6.4、#148）。
pub fn 部品カテゴリを検証する(value: &str) -> Result<(), Message> {
    閉じた語彙("category", value, PART_CATEGORIES)
}

/// 種別を検証する。**大文字・小文字を寄せずに拒否する**（#125）。
///
/// 旧表記の `Vpn` 等を黙って `VPN` に直すと、語彙が経路ごとに2通りになる。
/// 語彙外の値は既定へ倒さず拒否する（Q-21）のと同じ扱いにする。
pub fn 種別を検証する(value: &str) -> Result<(), Message> {
    閉じた語彙("device_category", value, DEVICE_CATEGORIES)
}

/// 重量を検証する。**正の整数か未指定**（24.2.1）。
pub fn 重量を検証する(項目: &str, value: Option<i32>) -> Result<(), Message> {
    match value {
        Some(v) if v <= 0 => Err(理由!(
            "import_detail.format.not_positive",
            field = 項目,
            value = v
        )),
        _ => Ok(()),
    }
}

/// 設備・什器の型番を検証する（12.10）。
///
/// **種別に合わない収容能力は拒否し、合う収容能力が無くても拒否する。**
/// 画面と同じ規則にする（8.6「手入力の画面と取込で扱いを揃える」）。
pub fn 設備の型番を検証する(m: &ContainerModelInput) -> Result<(), Message> {
    閉じた語彙("container_type", &m.container_type, CONTAINER_TYPES)?;
    if 正規化(&m.model_name).is_empty() {
        return Err(理由!("import_detail.format.model_name_empty"));
    }
    let is_rack = m.container_type == "Rack";
    let is_shelving = m.container_type == "Shelving";
    match (is_rack, m.height_u) {
        (true, None) => return Err(理由!("import_detail.format.rack_needs_height")),
        (false, Some(_)) => {
            return Err(理由!(
                "import_detail.format.height_only_rack",
                container_type = &m.container_type
            ))
        }
        _ => {}
    }
    match (is_shelving, m.shelf_count) {
        (true, None) => return Err(理由!("import_detail.format.shelving_needs_shelf_count")),
        (false, Some(_)) => {
            return Err(理由!(
                "import_detail.format.shelf_count_only_shelving",
                container_type = &m.container_type
            ))
        }
        _ => {}
    }
    for (項目, v) in [
        ("height_u", m.height_u),
        ("shelf_count", m.shelf_count),
        ("width_mm", m.width_mm),
        ("depth_mm", m.depth_mm),
        ("height_mm", m.height_mm),
        ("weight_g", m.weight_g),
        ("static_load_g", m.static_load_g),
    ] {
        重量を検証する(項目, v)?;
    }
    Ok(())
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
) -> Result<(Vec<展開後のポート>, Vec<Message>), Message> {
    let mut 出力 = Vec::new();
    let mut 警告 = Vec::new();

    for p in ports {
        // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
        if !PORT_KINDS.contains(&p.port_kind.as_str()) {
            return Err(理由!(
                "import_detail.format.port_kind_not_in_vocabulary",
                value = &p.port_kind
            ));
        }
        let connector = 正規化(&p.connector_type);
        if connector.is_empty() {
            return Err(理由!(
                "import_detail.format.connector_empty",
                port_kind = &p.port_kind
            ));
        }

        let labels = p.expand().map_err(|e| {
            理由!(
                "import_detail.format.port_prefixed",
                port_kind = &p.port_kind,
                reason = e
            )
        })?;

        // **意味を持たない列は捨てるが、黙って捨てない**（23.6、12.7）
        let port_speed = match p.port_speed.as_deref().map(正規化) {
            Some(s) if s.is_empty() => None,
            Some(s) if p.port_kind == NETWORK => Some(s),
            Some(s) => {
                警告.push(理由!(
                    "import_detail.format.port_speed_ignored",
                    value = s,
                    port_kind = &p.port_kind
                ));
                None
            }
            None => None,
        };

        let ratings = if p.power_ratings.is_empty() {
            Vec::new()
        } else if p.port_kind != POWER {
            警告.push(理由!(
                "import_detail.format.power_ratings_ignored",
                port_kind = &p.port_kind
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

fn 定格を検証する(ratings: &[PowerRatingInput]) -> Result<Vec<(String, i32, i32)>, Message> {
    let mut 出力: Vec<(String, i32, i32)> = Vec::new();

    for r in ratings {
        if !CURRENT_TYPES.contains(&r.current_type.as_str()) {
            return Err(理由!(
                "import_detail.format.current_type_not_in_vocabulary",
                value = &r.current_type
            ));
        }
        // **絶対値ではなく符号を含めた大小で見る**（12.7）。DCは -72 が下限
        if r.voltage_min > r.voltage_max {
            return Err(理由!(
                "import_detail.format.voltage_range",
                current_type = &r.current_type,
                min = r.voltage_min,
                max = r.voltage_max
            ));
        }
        // **方式ごとに1行**（12.7）。DBのUNIQUEに任せず、ここで理由を返す
        if 出力.iter().any(|(k, _, _)| *k == r.current_type) {
            return Err(理由!(
                "import_detail.format.current_type_duplicate",
                value = &r.current_type
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
pub fn vlanを検証する(v: &VlanInput) -> Result<Option<String>, Message> {
    if !(VLANタグの下限..=VLANタグの上限).contains(&v.vlan_tag) {
        return Err(理由!(
            "import_detail.format.vlan_tag_range",
            value = v.vlan_tag,
            min = VLANタグの下限,
            max = VLANタグの上限
        ));
    }
    if 正規化(&v.name).is_empty() {
        return Err(理由!("import_detail.format.name_empty"));
    }
    match v.zone.trim() {
        "" => Ok(None),
        z if ZONES.contains(&z) => Ok(Some(z.to_owned())),
        // **閉じた語彙は既定へ寄せず拒否する**（8.6）
        z => Err(理由!(
            "import_detail.format.zone_invalid",
            value = z,
            allowed = ZONES.join(" / ")
        )),
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

    /// **幅は `RackU` のときだけ。無ければ `Full`**（画面と同じ規則、#148）。
    #[test]
    fn 搭載形態と幅を検証する() {
        assert_eq!(
            搭載を検証する("RackU", None).unwrap().as_deref(),
            Some("Full")
        );
        assert_eq!(
            搭載を検証する("RackU", Some("Half")).unwrap().as_deref(),
            Some("Half")
        );
        assert_eq!(搭載を検証する("Surface", None).unwrap(), None);
        assert!(搭載を検証する("RackU", Some("half")).is_err());
        assert!(搭載を検証する("Surface", Some("Full")).is_err());
        assert!(搭載を検証する("racku", None).is_err());
    }

    #[test]
    fn スロット種別と部品カテゴリは閉じた語彙() {
        assert!(スロット("{ slot_type: Dimm, count: 2 }").expand().is_err());
        assert!(スロット("{ slot_type: DIMM, count: 2 }").expand().is_ok());
        assert!(部品カテゴリを検証する("PSU").is_ok());
        assert!(部品カテゴリを検証する("Psu").is_err());
        assert!(部品カテゴリを検証する("GPU").is_err());
    }

    fn 型番(yaml: &str) -> ContainerModelInput {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    /// **種別に合わない収容能力を拒否し、合う収容能力が無くても拒否する**（12.10）。
    #[test]
    fn 設備の型番は種別で収容能力の列が分かれる() {
        let base = "{ vendor: V, model_name: M, ";
        assert!(設備の型番を検証する(&型番(&format!(
            "{base}container_type: Rack, height_u: 42 }}"
        )))
        .is_ok());
        assert!(設備の型番を検証する(&型番(&format!("{base}container_type: Rack }}"))).is_err());
        assert!(設備の型番を検証する(&型番(&format!(
            "{base}container_type: Rack, height_u: 42, shelf_count: 5 }}"
        )))
        .is_err());
        assert!(設備の型番を検証する(&型番(&format!(
            "{base}container_type: Shelving, shelf_count: 5 }}"
        )))
        .is_ok());
        assert!(設備の型番を検証する(&型番(&format!("{base}container_type: Desk }}"))).is_ok());
        assert!(設備の型番を検証する(&型番(&format!(
            "{base}container_type: Desk, height_u: 1 }}"
        )))
        .is_err());
        assert!(設備の型番を検証する(&型番(&format!(
            "{base}container_type: rack, height_u: 42 }}"
        )))
        .is_err());
        assert!(設備の型番を検証する(&型番(&format!(
            "{base}container_type: Rack, height_u: 42, weight_g: 0 }}"
        )))
        .is_err());
    }

    /// **正規化は仮名・漢字を壊さない**（18.4）。
    #[test]
    fn 正規化は全角英数だけを直す() {
        assert_eq!(正規化("  ＨＰＥ  "), "HPE");
        assert_eq!(正規化("富士通　の　製品"), "富士通 の 製品");
    }
}

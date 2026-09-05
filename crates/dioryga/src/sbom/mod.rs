//! SBOMの取込（設計書9章）。
//!
//! # 2つのフォーマットを1つの中間表現に寄せる
//!
//! CycloneDXとSPDXは構造が違うが、**どちらもPURLでコンポーネントを識別する**
//! （9.1）。同じ中間表現へ正規化してから取り込めば、パーサーを2つ用意する
//! だけで済み、以降の処理（ハッシュ・差分・索引）は1本になる。
//!
//! # 順序が揺れると内容アドレス指定が壊れる
//!
//! 9.7の内容アドレス指定は「同一内容は1件しか保存しない」ことで成立している。
//! **同じ構成のSBOMが出力順の違いだけで別ハッシュになると、その前提が崩れる。**
//! 正規化の最後に必ず整列する（9.6の手順1）。
//!
//! # 保持しないものを決めておく
//!
//! `component.hashes` 等のメタデータは**捨てる**（9.6）。スナップショットが
//! 肥大するうえ、ツールの版で値が変わるとハッシュが不安定になる——**変わって
//! ほしくないもので識別しているのに、変わりやすいものを混ぜない。**
//!
//! # 取込は何も自動生成しない
//!
//! `SOFTWARE_INSTANCE` も `VENDOR` も作らない（9.6、18.3）。**観測記録であって
//! 資産の登録ではない。**サプライヤ名はスナップショット内に文字列として残す。

pub mod apply;
pub mod parse;

use serde::{Deserialize, Serialize};

/// 正規化後のコンポーネント1件（設計書9.6）。
///
/// **この構造体がそのままハッシュの対象になる。**フィールドを足すと既存の
/// スナップショットとハッシュが変わり、内容アドレス指定の共有が一度切れる。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub version: String,
    /// **差分計算時の同一性判定キー**（9.6）。無ければ `name` で代替する。
    pub purl: Option<String>,
    /// 文字列のまま保持する。**VENDORマスタには入れない**（18.3）。
    pub supplier: Option<String>,
    pub category: Option<String>,
    pub license_expression: Option<String>,
}

impl Component {
    /// 差分の同一性判定キー（設計書9.6）。
    ///
    /// **`purl` があればそれ、無ければ `name`。**版が変わっても同じキーになる
    /// ことが要点で、そうでないと `version_changed` を検出できない。
    fn 同一性キー(&self) -> &str {
        match &self.purl {
            Some(p) => 版を落とす(p),
            None => &self.name,
        }
    }
}

/// PURLから版を落とす。`pkg:maven/g/a@1.2.3` → `pkg:maven/g/a`。
///
/// **PURLは版を含む。**そのまま比較すると版が変わっただけで別物になり、
/// `version_changed` が `removed` + `added` に化ける。
fn 版を落とす(purl: &str) -> &str {
    match purl.rfind('@') {
        Some(i) => &purl[..i],
        None => purl,
    }
}

/// 正規化されたSBOM。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    pub components: Vec<Component>,
    /// CycloneDX / SPDX。
    pub source_format: &'static str,
}

impl Normalized {
    /// 正規化した内容のJSONとSHA-256を返す（設計書9.6の手順2）。
    ///
    /// **整列してから直列化する。**出力順の違いだけで別ハッシュになると、
    /// 内容アドレス指定が効かない。
    pub fn 内容(&self) -> (String, String) {
        use sha2::{Digest, Sha256};

        let mut sorted = self.components.clone();
        sorted.sort();
        sorted.dedup();

        // **内側の形式はJSONのまま**（9.7）。デバッグ容易性を優先する
        let json = serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".to_owned());

        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        // **16進で持つ。**主キーとして目で追える形にしておく（9.7）
        let hex = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        (json, hex)
    }
}

/// 差分の種類（設計書9.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum 変化 {
    Added,
    Removed,
    VersionChanged,
}

impl 変化 {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::VersionChanged => "version_changed",
        }
    }
}

/// 1件ぶんの差分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub change_type: 変化,
    pub name: String,
    pub purl: Option<String>,
    /// `added` のときは `None`。
    pub version_from: Option<String>,
    /// `removed` のときは `None`。
    pub version_to: Option<String>,
}

/// 直前の内容との差分を計算する（設計書9.6）。
///
/// **同一性の判定は `purl`、無ければ `name`。**同じキーで `version` だけが
/// 変わっていれば `version_changed`、片側にしか無ければ `added` / `removed`。
pub fn 差分(前: &[Component], 後: &[Component]) -> Vec<Change> {
    use std::collections::BTreeMap;

    let 索引 = |v: &[Component]| -> BTreeMap<String, Component> {
        v.iter()
            .map(|c| (c.同一性キー().to_owned(), c.clone()))
            .collect()
    };
    let 旧 = 索引(前);
    let 新 = 索引(後);

    let mut out = Vec::new();

    for (key, c) in &新 {
        match 旧.get(key) {
            None => out.push(Change {
                change_type: 変化::Added,
                name: c.name.clone(),
                purl: c.purl.clone(),
                version_from: None,
                version_to: Some(c.version.clone()),
            }),
            Some(old) if old.version != c.version => out.push(Change {
                change_type: 変化::VersionChanged,
                name: c.name.clone(),
                purl: c.purl.clone(),
                version_from: Some(old.version.clone()),
                version_to: Some(c.version.clone()),
            }),
            Some(_) => {}
        }
    }

    for (key, c) in &旧 {
        if !新.contains_key(key) {
            out.push(Change {
                change_type: 変化::Removed,
                name: c.name.clone(),
                purl: c.purl.clone(),
                version_from: Some(c.version.clone()),
                version_to: None,
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 部品(name: &str, version: &str, purl: Option<&str>) -> Component {
        Component {
            name: name.to_owned(),
            version: version.to_owned(),
            purl: purl.map(str::to_owned),
            supplier: None,
            category: None,
            license_expression: None,
        }
    }

    /// **順序が違っても同じハッシュになること**（設計書9.7）。
    ///
    /// ここが崩れると内容アドレス指定が成立しない。
    #[test]
    fn 順序が違っても同じ内容なら同じハッシュ() {
        let a = Normalized {
            components: vec![部品("openssl", "3.0.13", None), 部品("zlib", "1.3", None)],
            source_format: "CycloneDX",
        };
        let b = Normalized {
            components: vec![部品("zlib", "1.3", None), 部品("openssl", "3.0.13", None)],
            source_format: "CycloneDX",
        };
        assert_eq!(a.内容().1, b.内容().1);
    }

    /// 内容が違えば別のハッシュになること。
    #[test]
    fn 内容が違えば別のハッシュ() {
        let a = Normalized {
            components: vec![部品("openssl", "3.0.13", None)],
            source_format: "CycloneDX",
        };
        let b = Normalized {
            components: vec![部品("openssl", "3.0.14", None)],
            source_format: "CycloneDX",
        };
        assert_ne!(a.内容().1, b.内容().1);
    }

    /// **PURLの版を落として突き合わせること**（設計書9.6）。
    ///
    /// PURLは版を含むため、そのまま比較すると `version_changed` が
    /// `removed` + `added` に化ける。
    #[test]
    fn 版だけ変わればversion_changed() {
        let 前 = vec![部品(
            "log4j-core",
            "2.14.1",
            Some("pkg:maven/org/log4j@2.14.1"),
        )];
        let 後 = vec![部品(
            "log4j-core",
            "2.17.1",
            Some("pkg:maven/org/log4j@2.17.1"),
        )];

        let d = 差分(&前, &後);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change_type, 変化::VersionChanged);
        assert_eq!(d[0].version_from.as_deref(), Some("2.14.1"));
        assert_eq!(d[0].version_to.as_deref(), Some("2.17.1"));
    }

    /// `purl` が無ければ `name` で突き合わせること（設計書9.6）。
    #[test]
    fn purlが無ければ名前で突き合わせる() {
        let 前 = vec![部品("custom-agent", "1.0.0", None)];
        let 後 = vec![部品("custom-agent", "1.1.0", None)];

        let d = 差分(&前, &後);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change_type, 変化::VersionChanged);
    }

    #[test]
    fn 追加と削除を検出する() {
        let 前 = vec![部品("a", "1", None), 部品("b", "1", None)];
        let 後 = vec![部品("b", "1", None), 部品("c", "1", None)];

        let d = 差分(&前, &後);
        assert_eq!(d.len(), 2);
        assert!(d
            .iter()
            .any(|c| c.change_type == 変化::Added && c.name == "c"));
        assert!(d
            .iter()
            .any(|c| c.change_type == 変化::Removed && c.name == "a"));
    }

    /// **変化が無ければ差分は0件**（設計書9.6の手順3）。
    #[test]
    fn 変化が無ければ差分は空() {
        let v = vec![部品("a", "1", None)];
        assert!(差分(&v, &v).is_empty());
    }
}

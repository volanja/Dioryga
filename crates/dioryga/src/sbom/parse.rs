//! CycloneDX / SPDX のパース（設計書9.1、9.6）。
//!
//! # 形式は中身で見分ける
//!
//! 拡張子もMIMEタイプも当てにならない（どちらも `.json`）。**CycloneDXは
//! `bomFormat`、SPDXは `spdxVersion` を必ず持つ**ため、その有無で判別する。
//!
//! # XMLは扱わない
//!
//! CycloneDXはJSON/XMLの両方を持つが、**v1ではJSONだけを受け付ける。**
//! 実運用のツールチェーン（Syft、Trivy、cdxgen）はいずれもJSONを既定で出力し、
//! XMLパーサを1つ増やす費用に見合わない。**受け付けないことを明示的に伝える**
//! ——黙って0件として取り込むと、取り込めたように見えて中身が空になる。
//!
//! # 読めないフィールドは捨てる
//!
//! 9.6のマッピング表にあるものだけを拾い、残りは無視する。`component.hashes`
//! 等を保持しないのは、**スナップショットの肥大とハッシュの不安定化を避ける**
//! ため（9.6）。

use serde::Deserialize;

use super::{Component, Normalized};

/// 取込できなかった理由。**画面にそのまま出す。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// JSONとして読めない。
    NotJson,
    /// CycloneDXでもSPDXでもない。
    UnknownFormat,
    /// コンポーネントが1件も無い。
    Empty,
}

impl ParseError {
    pub fn key(self) -> &'static str {
        match self {
            Self::NotJson => "sbom.error_not_json",
            Self::UnknownFormat => "sbom.error_unknown_format",
            Self::Empty => "sbom.error_empty",
        }
    }
}

pub fn parse(bytes: &[u8]) -> Result<Normalized, ParseError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ParseError::NotJson)?;

    // **中身で見分ける。**拡張子もMIMEタイプも当てにならない
    let normalized = if value.get("bomFormat").is_some() || value.get("components").is_some() {
        cyclonedx(&value)?
    } else if value.get("spdxVersion").is_some() || value.get("packages").is_some() {
        spdx(&value)?
    } else {
        return Err(ParseError::UnknownFormat);
    };

    if normalized.components.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(normalized)
}

// ---------------------------------------------------------------------------
// CycloneDX
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CycloneComponent {
    name: Option<String>,
    version: Option<String>,
    purl: Option<String>,
    /// `application` / `library` / `operating-system` 等（9.1）。
    #[serde(rename = "type")]
    kind: Option<String>,
    supplier: Option<CycloneSupplier>,
    #[serde(default)]
    licenses: Vec<CycloneLicense>,
}

#[derive(Debug, Deserialize)]
struct CycloneSupplier {
    name: Option<String>,
}

/// `licenses` は `{license: {id|name}}` か `{expression}` のどちらかで来る。
#[derive(Debug, Deserialize)]
struct CycloneLicense {
    license: Option<CycloneLicenseBody>,
    expression: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CycloneLicenseBody {
    id: Option<String>,
    name: Option<String>,
}

fn cyclonedx(value: &serde_json::Value) -> Result<Normalized, ParseError> {
    let Some(list) = value.get("components").and_then(|v| v.as_array()) else {
        return Ok(Normalized {
            components: Vec::new(),
            source_format: "CycloneDX",
        });
    };

    let components = list
        .iter()
        .filter_map(|v| serde_json::from_value::<CycloneComponent>(v.clone()).ok())
        .filter_map(|c| {
            // **名前の無いコンポーネントは捨てる。**同一性の判定にも表示にも
            // 使えず、残すとハッシュだけが揺れる
            let name = 空でない(c.name)?;
            Some(Component {
                name,
                version: c.version.unwrap_or_default(),
                purl: 空でない(c.purl),
                supplier: c.supplier.and_then(|s| 空でない(s.name)),
                category: 空でない(c.kind),
                license_expression: c.licenses.first().and_then(ライセンス),
            })
        })
        .collect();

    Ok(Normalized {
        components,
        source_format: "CycloneDX",
    })
}

fn ライセンス(l: &CycloneLicense) -> Option<String> {
    if let Some(e) = &l.expression {
        return 空でない(Some(e.clone()));
    }
    let body = l.license.as_ref()?;
    空でない(body.id.clone().or_else(|| body.name.clone()))
}

// ---------------------------------------------------------------------------
// SPDX
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SpdxPackage {
    name: Option<String>,
    #[serde(rename = "versionInfo")]
    version_info: Option<String>,
    supplier: Option<String>,
    #[serde(rename = "primaryPackagePurpose")]
    purpose: Option<String>,
    #[serde(rename = "licenseConcluded")]
    license_concluded: Option<String>,
    #[serde(rename = "externalRefs", default)]
    external_refs: Vec<SpdxExternalRef>,
}

#[derive(Debug, Deserialize)]
struct SpdxExternalRef {
    #[serde(rename = "referenceType")]
    reference_type: Option<String>,
    #[serde(rename = "referenceLocator")]
    reference_locator: Option<String>,
}

fn spdx(value: &serde_json::Value) -> Result<Normalized, ParseError> {
    let Some(list) = value.get("packages").and_then(|v| v.as_array()) else {
        return Ok(Normalized {
            components: Vec::new(),
            source_format: "SPDX",
        });
    };

    let components = list
        .iter()
        .filter_map(|v| serde_json::from_value::<SpdxPackage>(v.clone()).ok())
        .filter_map(|p| {
            let name = 空でない(p.name)?;
            Some(Component {
                name,
                version: p.version_info.unwrap_or_default(),
                // **`externalRefs` から `purl` を拾う**（9.6のマッピング表）
                purl: p
                    .external_refs
                    .iter()
                    .find(|r| r.reference_type.as_deref() == Some("purl"))
                    .and_then(|r| 空でない(r.reference_locator.clone())),
                // SPDXは値が無いとき `NOASSERTION` を入れる。空と同じ扱いにする
                supplier: 空でない(p.supplier).filter(|s| s != "NOASSERTION"),
                category: 空でない(p.purpose),
                license_expression: 空でない(p.license_concluded)
                    .filter(|s| s != "NOASSERTION" && s != "NONE"),
            })
        })
        .collect();

    Ok(Normalized {
        components,
        source_format: "SPDX",
    })
}

/// 空文字と空白だけの値を `None` に寄せる。
///
/// **`Some("")` と `None` を混在させない。**混ざると同じ内容が別ハッシュに
/// なりうる（9.7の内容アドレス指定が効かなくなる）。
fn 空でない(value: Option<String>) -> Option<String> {
    let v = value?;
    let t = v.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CYCLONEDX: &str = r#"{
      "bomFormat": "CycloneDX",
      "specVersion": "1.5",
      "components": [
        {
          "type": "library",
          "name": "openssl",
          "version": "3.0.13",
          "purl": "pkg:generic/openssl@3.0.13",
          "supplier": { "name": "OpenSSL Project" },
          "licenses": [{ "license": { "id": "Apache-2.0" } }],
          "hashes": [{ "alg": "SHA-256", "content": "deadbeef" }]
        },
        { "type": "application", "name": "nginx", "version": "1.24.0" }
      ]
    }"#;

    const SPDX: &str = r#"{
      "spdxVersion": "SPDX-2.3",
      "packages": [
        {
          "name": "openssl",
          "versionInfo": "3.0.13",
          "supplier": "NOASSERTION",
          "licenseConcluded": "Apache-2.0",
          "primaryPackagePurpose": "LIBRARY",
          "externalRefs": [
            { "referenceType": "purl", "referenceLocator": "pkg:generic/openssl@3.0.13" }
          ]
        }
      ]
    }"#;

    #[test]
    fn cyclonedxを読める() {
        let n = parse(CYCLONEDX.as_bytes()).unwrap();
        assert_eq!(n.source_format, "CycloneDX");
        assert_eq!(n.components.len(), 2);

        let openssl = n.components.iter().find(|c| c.name == "openssl").unwrap();
        assert_eq!(openssl.version, "3.0.13");
        assert_eq!(openssl.purl.as_deref(), Some("pkg:generic/openssl@3.0.13"));
        assert_eq!(openssl.supplier.as_deref(), Some("OpenSSL Project"));
        assert_eq!(openssl.license_expression.as_deref(), Some("Apache-2.0"));
    }

    /// **`hashes` 等のメタデータを保持しないこと**（設計書9.6）。
    ///
    /// ツールの版で値が変わるとハッシュが不安定になる。
    #[test]
    fn 余分なメタデータは保持しない() {
        let n = parse(CYCLONEDX.as_bytes()).unwrap();
        let (json, _) = n.内容();
        assert!(!json.contains("deadbeef"), "hashes が残っている");
        assert!(!json.contains("SHA-256"));
    }

    #[test]
    fn spdxを読める() {
        let n = parse(SPDX.as_bytes()).unwrap();
        assert_eq!(n.source_format, "SPDX");
        assert_eq!(n.components.len(), 1);

        let c = &n.components[0];
        assert_eq!(c.purl.as_deref(), Some("pkg:generic/openssl@3.0.13"));
        // **`NOASSERTION` は空と同じ扱い**
        assert!(c.supplier.is_none());
        assert_eq!(c.license_expression.as_deref(), Some("Apache-2.0"));
    }

    /// **同じ構成なら形式が違っても同じハッシュになること**（設計書9.1、9.7）。
    ///
    /// 中間表現に寄せた意味がここに出る。CycloneDXで取ったサーバとSPDXで取った
    /// サーバが、実は同じイメージだった、という場合にスナップショットを共有できる。
    #[test]
    fn 形式が違っても内容が同じならハッシュが一致する() {
        let cdx = r#"{"bomFormat":"CycloneDX","components":[
          {"name":"openssl","version":"3.0.13","purl":"pkg:generic/openssl@3.0.13",
           "licenses":[{"license":{"id":"Apache-2.0"}}],"type":"LIBRARY"}]}"#;
        let spdx = r#"{"spdxVersion":"SPDX-2.3","packages":[
          {"name":"openssl","versionInfo":"3.0.13","licenseConcluded":"Apache-2.0",
           "primaryPackagePurpose":"LIBRARY",
           "externalRefs":[{"referenceType":"purl","referenceLocator":"pkg:generic/openssl@3.0.13"}]}]}"#;

        assert_eq!(
            parse(cdx.as_bytes()).unwrap().内容().1,
            parse(spdx.as_bytes()).unwrap().内容().1
        );
    }

    #[test]
    fn 読めないものは理由を返す() {
        assert_eq!(parse(b"not json").unwrap_err(), ParseError::NotJson);
        assert_eq!(
            parse(b"{\"foo\":1}").unwrap_err(),
            ParseError::UnknownFormat
        );
        assert_eq!(
            parse(br#"{"bomFormat":"CycloneDX","components":[]}"#).unwrap_err(),
            ParseError::Empty
        );
    }

    /// 名前の無いコンポーネントは捨てること。
    #[test]
    fn 名前が無いものは捨てる() {
        let json = r#"{"bomFormat":"CycloneDX","components":[
          {"version":"1.0"},{"name":"ok","version":"1.0"}]}"#;
        let n = parse(json.as_bytes()).unwrap();
        assert_eq!(n.components.len(), 1);
        assert_eq!(n.components[0].name, "ok");
    }
}

//! 一括取込（設計書23章）。
//!
//! # 命令ではなく「あるべき現在の状態」を書く（23.1）
//!
//! 取込ファイルは操作の列挙ではなく、**「現在こうなっているはず」という宣言**
//! である。取込処理は現在の事実と突き合わせ、**差分だけを書き込む。**
//!
//! 命令形にすると、同じファイルを2回流したときに履歴行が重複して事実が壊れる。
//! 宣言形なら**何度流しても結果が変わらず**、「1行直して再取込」という現実的な
//! 運用ができる。
//!
//! # 必ずドライランを挟む（23.6）
//!
//! 18.2により参照されたカタログ行は編集できない。**誤った取込は事後修正が
//! 困難**なため、差分レポートを見てから実行する2段階にする。
//!
//! # 取込では行ごとの監査ログを書かない（24.4）
//!
//! [`Actor::Import`] を使う。25万行の取込で25万行の監査ログが生まれると
//! 22.5の肥大問題を悪化させるうえ、すべて同じ主体・時刻・`import_run_id` を
//! 持つため情報が増えない。追跡は `IMPORT_RUN` が担う。

pub mod catalog;
pub mod costs;
pub mod instances;
pub mod network;
pub mod organization;
pub mod parts;
pub mod placement;
pub mod run;
pub mod workflow;

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

/// 1件ごとの判定（設計書23.6）。
///
/// **警告とエラーを分けるのが要点。**実機が仕様の想定外であること自体はあり、
/// 誤って拒否すると事実を記録できなくなる（不変条件6）。一方、参照先が
/// 解決できない・必須列が無いといった**解釈できない入力は取り込まない。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    /// 現在の事実と一致している。**履歴行を作らない。**
    Unchanged,
    Created,
    Updated,
    /// 取り込むが記録する。
    Warning,
    /// 取り込まない。
    Error,
}

impl Outcome {
    /// 判定の表示名。画面は利用者の言語、コンソールはOSの言語で呼ぶ（#191）。
    pub fn 文言(self, l: &str) -> String {
        let key = match self {
            Self::Unchanged => "import.outcome_unchanged",
            Self::Created => "import.outcome_created",
            Self::Updated => "import.outcome_updated",
            Self::Warning => "import.outcome_warning",
            Self::Error => "import.outcome_error",
        };
        rust_i18n::t!(key, locale = l).into_owned()
    }
}

/// 差分レポートの1行。
#[derive(Debug, Clone)]
pub struct Entry {
    pub outcome: Outcome,
    /// 対象を人が読める形で示す。自然キーをそのまま使う（23.2）。
    pub target: String,
    /// 警告・エラーの理由。利用者が直せる言葉で書く。
    pub detail: String,
}

impl Entry {
    pub fn new(outcome: Outcome, target: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            target: target.into(),
            detail: detail.into(),
        }
    }
}

/// 差分レポート（設計書23.6）。
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub entries: Vec<Entry>,
}

impl Report {
    pub fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
    }

    pub fn count(&self, outcome: Outcome) -> usize {
        self.entries.iter().filter(|e| e.outcome == outcome).count()
    }

    /// **1件でもエラーがあれば取り込まない。**
    ///
    /// 部分的に適用すると、どこまで入ったのかを利用者が把握できない。
    /// 23.6が2段階にしている狙い（誤った取込を未然に防ぐ）とも合わない。
    pub fn has_error(&self) -> bool {
        self.entries.iter().any(|e| e.outcome == Outcome::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.outcome == Outcome::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(|e| e.outcome == Outcome::Warning)
    }
}

impl Report {
    /// 判定ごとの件数の要約（#191）。
    pub fn 集計(&self, l: &str) -> String {
        rust_i18n::t!(
            "import.summary_counts",
            locale = l,
            created = self.count(Outcome::Created),
            updated = self.count(Outcome::Updated),
            unchanged = self.count(Outcome::Unchanged),
            warning = self.count(Outcome::Warning),
            error = self.count(Outcome::Error)
        )
        .into_owned()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.集計(crate::console::言語()))
    }
}

/// 取り込んだファイルのSHA-256（設計書23.7）。
///
/// 同じファイルを二度流したかを後から判別するために `IMPORT_RUN` へ残す。
pub fn file_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// **文言は言語ごとに訳す**（#191）。画面は利用者の言語で [`Self::文言`] を呼び、
/// コンソール（`Display`）は [`crate::console::言語`] で出す。
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{}", self.文言(crate::console::言語()))]
    Io(#[from] std::io::Error),

    #[error("{}", self.文言(crate::console::言語()))]
    Yaml(#[from] serde_yaml_ng::Error),

    #[error("{}", self.文言(crate::console::言語()))]
    Csv(String),

    #[error("{}", self.文言(crate::console::言語()))]
    UnsupportedVersion { found: u32, expected: u32 },

    #[error("{}", self.文言(crate::console::言語()))]
    UnexpectedKind { found: String, expected: String },

    #[error("{}", self.文言(crate::console::言語()))]
    HasErrors(usize),

    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

impl ImportError {
    pub fn 文言(&self, l: &str) -> String {
        use rust_i18n::t;
        match self {
            Self::Io(e) => t!("errors.import_io", locale = l, detail = e),
            Self::Yaml(e) => t!("errors.import_yaml", locale = l, detail = e),
            Self::Csv(detail) => t!("errors.import_csv", locale = l, detail = detail),
            Self::UnsupportedVersion { found, expected } => t!(
                "errors.import_version",
                locale = l,
                found = found,
                expected = expected
            ),
            Self::UnexpectedKind { found, expected } => t!(
                "errors.import_kind",
                locale = l,
                found = found,
                expected = expected
            ),
            Self::HasErrors(count) => t!("errors.import_has_errors", locale = l, count = count),
            Self::Db(e) => return e.to_string(),
        }
        .into_owned()
    }
}

/// **形式の誤りは別crateが持つ**（#77）。同じ意味の変種へ写して受ける。
///
/// 変種を統合せず写すのは、`instances.rs`（CSVとマニフェスト）が同じ変種を
/// 使っており、そちらは形式のcrateを通らないためである。
impl From<dioryga_catalog_format::FormatError> for ImportError {
    fn from(e: dioryga_catalog_format::FormatError) -> Self {
        use dioryga_catalog_format::FormatError as F;
        match e {
            F::Yaml(e) => Self::Yaml(e),
            F::UnsupportedVersion { found, expected } => {
                Self::UnsupportedVersion { found, expected }
            }
            F::UnexpectedKind { found, expected } => Self::UnexpectedKind { found, expected },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同じ内容なら同じハッシュになる() {
        assert_eq!(file_hash(b"abc"), file_hash(b"abc"));
        assert_ne!(file_hash(b"abc"), file_hash(b"abd"));
    }

    #[test]
    fn エラーが1件でもあれば取り込まない() {
        let mut report = Report::default();
        report.push(Entry::new(Outcome::Created, "a", ""));
        assert!(!report.has_error());

        report.push(Entry::new(Outcome::Warning, "b", "スロット超過"));
        assert!(!report.has_error(), "警告で止めてはならない");

        report.push(Entry::new(Outcome::Error, "c", "参照先が無い"));
        assert!(report.has_error());
    }

    #[test]
    fn 集計が判定ごとに分かれる() {
        let mut report = Report::default();
        report.push(Entry::new(Outcome::Created, "a", ""));
        report.push(Entry::new(Outcome::Created, "b", ""));
        report.push(Entry::new(Outcome::Unchanged, "c", ""));

        assert_eq!(report.count(Outcome::Created), 2);
        assert_eq!(report.count(Outcome::Unchanged), 1);
        assert_eq!(report.count(Outcome::Updated), 0);
    }
}

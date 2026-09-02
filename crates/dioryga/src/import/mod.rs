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
pub mod run;

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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "変更なし",
            Self::Created => "新規",
            Self::Updated => "更新",
            Self::Warning => "警告",
            Self::Error => "エラー",
        }
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

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "新規 {} 件 / 更新 {} 件 / 変更なし {} 件 / 警告 {} 件 / エラー {} 件",
            self.count(Outcome::Created),
            self.count(Outcome::Updated),
            self.count(Outcome::Unchanged),
            self.count(Outcome::Warning),
            self.count(Outcome::Error),
        )
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

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("ファイルを読み取れません: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAMLの形式が正しくありません: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),

    #[error("format_version が {found} です。対応しているのは {expected} です")]
    UnsupportedVersion { found: u32, expected: u32 },

    #[error("kind が {found} です。このコマンドが扱うのは {expected} です")]
    UnexpectedKind { found: String, expected: String },

    #[error("取り込めない内容が {0} 件あります")]
    HasErrors(usize),

    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
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

//! 画面から出るリンクが、実在するルートを指していることの検証（#92）。
//!
//! # なぜ要るか
//!
//! **存在しないパスへの `href` を描画しても、描画そのものは成功する。**結合
//! テストは各画面が200を返すことを見ているが、その画面が置いたリンクの先までは
//! 追わない。**リンク切れは、押されるまで誰も気付かない。**
//!
//! `Dioryga_Design` の24.6でブラウザE2Eを持たないと決めた際、結合テストで
//! 覆えないものとして残った2点のうち、**機械的に押さえられるのがこちら**である
//! （もう一方のフォームの `name` 属性のずれは目視に委ねる）。
//!
//! **不具合の修正ではなく、保証を作るテストである。**書いた時点でリンク切れは
//! 0件だった。v1の画面は段階的に増えており、**未実装の画面へのリンクを先に
//! 書いてしまう事故**が起こりやすい状態にある。
//!
//! # なぜ実際に叩かないのか
//!
//! ルータへ順にリクエストを投げて404でないことを見る形も考えたが、採らなかった。
//! **axumの `Router` は登録済みのパスを列挙する手段を公開していない**ため、
//! いずれにせよソースから取り出す必要がある。そして**ハンドラが返す404と、
//! ルート未登録の404は区別が付かない**——「機器が存在しない」で404を返す画面が
//! いくつもあり、リンク切れと見分けられない。
//!
//! DBもブラウザも要らない代わりに、**ソースの形に依存する。**抽出が空振り
//! したときに黙って通らないよう、件数の下限を検査している。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// ルートを定義しているファイル。**ここに全ルートが集まっていることが前提**で、
/// [`ルートは一箇所に集まっている`] が検査する。
const ルータ: &str = "src/server/mod.rs";

// ---------------------------------------------------------------------------
// 検証
// ---------------------------------------------------------------------------

/// **テンプレートの `href` / `action` が、実在するルートを指すこと。**
#[test]
fn 画面のリンクは実在する() {
    let routes = ルート一覧();
    let mut 切れ = Vec::new();

    for path in ファイル一覧("templates", "html") {
        let src = std::fs::read_to_string(&path).unwrap();
        for (属性, link) in リンクを拾う(&src) {
            if !一致する(&routes, &link) {
                切れ.push(format!(
                    "{}: {}=\"{}\"",
                    path.file_name().unwrap().to_string_lossy(),
                    属性,
                    link
                ));
            }
        }
    }

    assert!(
        切れ.is_empty(),
        "ルータに無いパスを指しています:\n  {}",
        切れ.join("\n  ")
    );
}

/// **リダイレクト先も、実在するルートを指すこと。**
///
/// 保存後の遷移先が消えていても、テストは`303`を確かめるだけで通ってしまう。
/// テンプレートのリンクと同じ種類の欠陥である。
#[test]
fn リダイレクト先は実在する() {
    let routes = ルート一覧();
    let mut 切れ = Vec::new();

    for path in ファイル一覧("src", "rs") {
        let src = std::fs::read_to_string(&path).unwrap();
        for link in リダイレクトを拾う(&src) {
            if !一致する(&routes, &link) {
                切れ.push(format!(
                    "{}: {}",
                    path.file_name().unwrap().to_string_lossy(),
                    link
                ));
            }
        }
    }

    assert!(
        切れ.is_empty(),
        "ルータに無いパスへリダイレクトしています:\n  {}",
        切れ.join("\n  ")
    );
}

/// **抽出が空振りしたまま通らないこと。**
///
/// このテストはソースの形に依存する。`.route(` の書き方が変わって1件も
/// 拾えなくなると、**リンクは何とでも一致するようになり、上の2つが
/// 意味を失う。**下限を置いて気付けるようにする。
#[test]
fn 抽出そのものが機能している() {
    let routes = ルート一覧();
    assert!(
        routes.len() > 50,
        "ルートを{}件しか拾えていません。抽出が壊れている可能性があります",
        routes.len()
    );

    let リンク数: usize = ファイル一覧("templates", "html")
        .iter()
        .map(|p| リンクを拾う(&std::fs::read_to_string(p).unwrap()).len())
        .sum();
    assert!(
        リンク数 > 50,
        "リンクを{リンク数}件しか拾えていません。抽出が壊れている可能性があります"
    );
}

/// **ルートが `mod.rs` の外に増えていないこと。**
///
/// 外で登録されたルートは [`ルート一覧`] に入らない。**入らないぶんには
/// 「そんなルートは無い」と誤検出するだけで、見逃しにはならない**が、
/// 通らないテストの原因が分かりにくくなる。増やすならこのテストも直す。
#[test]
fn ルートは一箇所に集まっている() {
    let mut 外 = Vec::new();
    for path in ファイル一覧("src", "rs") {
        if path.ends_with(ルータ.replace('/', std::path::MAIN_SEPARATOR_STR)) {
            continue;
        }
        if std::fs::read_to_string(&path).unwrap().contains(".route(") {
            外.push(path.display().to_string());
        }
    }
    assert!(
        外.is_empty(),
        "{ルータ} の外でルートを登録しています:\n  {}",
        外.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// 抽出
// ---------------------------------------------------------------------------

/// 登録済みのルートを、`.route(` に続く文字列リテラルから取り出す。
///
/// **`.route(` を起点にする。**ファイル中の `/` で始まる文字列を無差別に
/// 拾うと、リダイレクト先などが混ざって**ルートの集合が実際より広くなり、
/// リンク切れを見逃す。**
fn ルート一覧() -> BTreeSet<String> {
    let src = std::fs::read_to_string(crate_dir().join(ルータ)).unwrap();
    let mut out = BTreeSet::new();
    let mut 残り = src.as_str();

    while let Some(i) = 残り.find(".route(") {
        残り = &残り[i + ".route(".len()..];
        // 改行を挟んで次の行にパスが来る書き方がある
        let Some(開始) = 残り.find('"') else {
            break;
        };
        let 中身 = &残り[開始 + 1..];
        let Some(終了) = 中身.find('"') else {
            break;
        };
        out.insert(正規化(&中身[..終了]));
        残り = &中身[終了..];
    }
    out
}

/// テンプレートから `href` / `action` の値を取り出す。
///
/// **`/` で始まるものだけを見る。**`{{ row.href }}` のように値ごと変数の
/// ものは、ここでは判定できない。
fn リンクを拾う(src: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for 属性 in ["href", "action"] {
        let 印 = format!("{属性}=\"");
        let mut 残り = src;
        while let Some(i) = 残り.find(&印) {
            残り = &残り[i + 印.len()..];
            let Some(終了) = 残り.find('"') else {
                break;
            };
            let 値 = &残り[..終了];
            残り = &残り[終了..];
            if 値.starts_with('/') {
                out.push((属性, 正規化(値)));
            }
        }
    }
    out
}

/// `Redirect::to("...")` と `Redirect::to(&format!("...")` の行き先を取り出す。
fn リダイレクトを拾う(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut 残り = src;
    while let Some(i) = 残り.find("Redirect::to(") {
        残り = &残り[i + "Redirect::to(".len()..];
        let Some(開始) = 残り.find('"') else {
            break;
        };
        // 開き括弧から文字列までの間に `)` があるなら、それは別の呼び出し
        if 残り[..開始].contains(')') {
            continue;
        }
        let 中身 = &残り[開始 + 1..];
        let Some(終了) = 中身.find('"') else {
            break;
        };
        let 値 = &中身[..終了];
        残り = &中身[終了..];
        if 値.starts_with('/') {
            out.push(正規化(値));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 突き合わせ
// ---------------------------------------------------------------------------

/// パスパラメータを `{}` に均し、クエリと素片を落とす。
///
/// ルータ側の `{id}` / `{device_id}`、`format!` の `{project_id}`、Askamaの
/// `{{ row.id }}` は**いずれも「1セグメントの可変部分」**であり、名前が違って
/// いても同じものとして扱う。**名前で突き合わせると、ルータが `{id}` と
/// 呼んでいる箇所をテンプレートが `{{ project_id }}` で埋めているだけで
/// 落ちる。**
fn 正規化(値: &str) -> String {
    let 値 = 値.split(['?', '#']).next().unwrap_or(値);

    let mut out = String::new();
    let mut 残り = 値;
    while let Some(i) = 残り.find('{') {
        out.push_str(&残り[..i]);
        let 後ろ = &残り[i..];
        // Askamaの `{{ ... }}` とルータ・format! の `{ ... }` を同じに扱う
        let Some(終了) = 後ろ.find('}') else {
            out.push_str(後ろ);
            return out;
        };
        let 中 = &後ろ[..終了];
        if 中.starts_with("{*") {
            // ワイルドカード。ここから先は何にでも一致する
            out.push_str("{*}");
        } else {
            out.push_str("{}");
        }
        残り = &後ろ[終了 + 1..];
        // `}}` の閉じをもう1つ食う
        if let Some(次) = 残り.strip_prefix('}') {
            残り = 次;
        }
    }
    out.push_str(残り);
    out
}

/// 正規化済みのリンクが、いずれかのルートに当たるか。
fn 一致する(routes: &BTreeSet<String>, link: &str) -> bool {
    routes.iter().any(|r| {
        if let Some(prefix) = r.split_once("{*}") {
            // `/assets/{*path}` は配下のすべてに一致する
            return link.starts_with(prefix.0);
        }
        r == link
    })
}

// ---------------------------------------------------------------------------
// 走査
// ---------------------------------------------------------------------------

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// クレート直下の `dir` を再帰して、拡張子が `ext` のファイルを集める。
fn ファイル一覧(dir: &str, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    集める(&crate_dir().join(dir), ext, &mut out);
    out.sort();
    assert!(!out.is_empty(), "{dir} に .{ext} が見つかりません");
    out
}

fn 集める(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            集める(&p, ext, out);
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

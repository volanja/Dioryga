//! コンソール（標準出力）への書き出しと、そこに出す言語（#193、#191）。
//!
//! # 読み手が先に閉じても panic しない
//!
//! `dioryga licenses | less` で `less` を閉じると、残りを書こうとした時点で
//! 読み手がいなくなっている。Rust のプログラムは SIGPIPE を無視する設定で
//! 起動するため、書き込みはシグナルで止まらず `BrokenPipe` の誤りとして返り、
//! **`print!` はこれを panic に変える。**
//!
//! 読むのをやめたのは利用者の操作であって、誤りではない。**`BrokenPipe` は
//! 正常な終わりとして扱う。**
//!
//! SIGPIPE の扱いを既定に戻す案（`libc::signal(SIGPIPE, SIG_DFL)`）は Unix に
//! しか無く、Windows で同じ問題を解決できない。誤りの種別で扱えば、両方の OS で
//! 同じコードになる。

use std::io::{self, Write};
use std::sync::OnceLock;

/// コンソールに出す言語（`"ja"` か `"en"`、#191）。
///
/// # OSのロケールから決める
///
/// コンソールには利用者の言語設定が無い（初回セットアップの時点では利用者が
/// 存在しない）。設定ファイルに項目を足す案は、設定を読む前に出す `--help` や
/// `licenses` に効かないので採らない。
///
/// POSIX の慣習どおり `LC_ALL` → `LC_MESSAGES` → `LANG` の順に見て、どれも
/// 無ければ OS の表示言語を見る（Windowsは `LANG` を持たないことが多い）。
/// **日本語と判断できなければ英語にする**（README と同じ既定）。
pub fn 言語() -> &'static str {
    static 言語: OnceLock<&'static str> = OnceLock::new();
    言語.get_or_init(|| 言語を決める(|name| std::env::var(name).ok(), sys_locale::get_locale))
}

fn 言語を決める(
    env: impl Fn(&str) -> Option<String>,
    os: impl FnOnce() -> Option<String>,
) -> &'static str {
    let 指定 = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(env)
        .find(|v| !v.is_empty());
    // 環境変数で指定があれば、それに従う（`C` や `en_US` なら英語）
    match 指定.or_else(os) {
        Some(v) if v.starts_with("ja") => "ja",
        _ => "en",
    }
}

/// 標準出力へ書き出す。読み手が先に閉じていれば、何もせず `Ok` を返す。
pub fn 書く(text: &str) -> io::Result<()> {
    書き出す(&mut io::stdout().lock(), text)
}

fn 書き出す(out: &mut impl Write, text: &str) -> io::Result<()> {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 指定した種別の誤りを返し続ける書き出し先。
    struct 閉じた(io::ErrorKind);

    impl Write for 閉じた {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(self.0))
        }
    }

    #[test]
    fn 読み手が閉じていても誤りにしない() {
        assert!(書き出す(&mut 閉じた(io::ErrorKind::BrokenPipe), "本文").is_ok());
    }

    /// **`BrokenPipe` 以外は握りつぶさない。**書けなかったことを黙ると、
    /// 出力が欠けたまま正常終了する。
    #[test]
    fn ほかの誤りはそのまま返す() {
        let e = 書き出す(&mut 閉じた(io::ErrorKind::PermissionDenied), "本文").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    }

    fn 決める(vars: &[(&str, &str)], os: Option<&str>) -> &'static str {
        言語を決める(
            |name| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            },
            || os.map(str::to_owned),
        )
    }

    #[test]
    fn 環境変数が日本語なら日本語にする() {
        assert_eq!(決める(&[("LANG", "ja_JP.UTF-8")], None), "ja");
    }

    /// **`LC_ALL` が `LANG` より優先する**（POSIX の順）。
    #[test]
    fn 環境変数は優先順に見る() {
        assert_eq!(
            決める(&[("LC_ALL", "en_US.UTF-8"), ("LANG", "ja_JP.UTF-8")], None),
            "en"
        );
        assert_eq!(
            決める(&[("LC_MESSAGES", "ja_JP.UTF-8"), ("LANG", "C")], None),
            "ja"
        );
        // 空の変数は指定が無いものとして飛ばす
        assert_eq!(
            決める(&[("LC_ALL", ""), ("LANG", "ja_JP.UTF-8")], None),
            "ja"
        );
    }

    /// 環境変数で指定があれば、OS の表示言語より優先する。
    #[test]
    fn 環境変数の指定はosより優先する() {
        assert_eq!(決める(&[("LANG", "C")], Some("ja-JP")), "en");
    }

    /// Windows では `LANG` が無いことが多い。OS の表示言語を見る。
    #[test]
    fn 環境変数が無ければosの表示言語を見る() {
        assert_eq!(決める(&[], Some("ja-JP")), "ja");
        assert_eq!(決める(&[], Some("en-US")), "en");
    }

    #[test]
    fn 決められなければ英語にする() {
        assert_eq!(決める(&[], None), "en");
        assert_eq!(決める(&[("LANG", "fr_FR.UTF-8")], None), "en");
    }

    #[test]
    fn 書けるときはそのまま書く() {
        let mut out = Vec::new();
        書き出す(&mut out, "本文").unwrap();
        assert_eq!(out, "本文".as_bytes());
    }
}

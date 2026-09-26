//! コンソール（標準出力）への書き出し（#193）。
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

    #[test]
    fn 書けるときはそのまま書く() {
        let mut out = Vec::new();
        書き出す(&mut out, "本文").unwrap();
        assert_eq!(out, "本文".as_bytes());
    }
}

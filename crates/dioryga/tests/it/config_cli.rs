//! `-c` で指定した設定ファイルの扱いの結合テスト（設計書24.1、#189）。
//!
//! 設定の読み込みは単体テストで確かめている。ここでは、**打ち間違えたパスで
//! 起動しようとしたとき、既定値のSQLiteを作らずに止まること**を、バイナリを
//! 起動して確かめる。問題は「気付かないまま別のDBに記録が入る」ことであり、
//! ファイルが作られないことまで見ないと確かめたことにならない。

use std::process::Command;

/// 空の作業ディレクトリを作る。既定の接続先（`dioryga.db`）はここに作られる。
fn 作業場所() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dioryga-config-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn dioryga(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dioryga"));
    cmd.current_dir(dir);
    // 手元やCIの環境変数で接続先が変わらないようにする
    for (key, _) in std::env::vars() {
        if key.starts_with("DIORYGA_") {
            cmd.env_remove(key);
        }
    }
    cmd
}

/// **明示したファイルが無ければ、パスを示して止まり、SQLiteを作らない。**
#[test]
fn 明示した設定ファイルが無ければ止まる() {
    let dir = 作業場所();
    let output = dioryga(&dir)
        .args(["-c", "typo.toml", "migrate"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "止まっていません: {stderr}");
    assert!(
        stderr.contains("typo.toml"),
        "パスが示されていません: {stderr}"
    );
    assert!(
        !dir.join("dioryga.db").exists(),
        "既定値のSQLiteが作られています"
    );
}

/// 明示しなければ、設定ファイルが無くても既定値で動く（従来どおり）。
#[test]
fn 設定ファイルを明示しなければ既定値で動く() {
    let dir = 作業場所();
    let output = dioryga(&dir).arg("migrate").output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "失敗しました: {stderr}");
    assert!(
        dir.join("dioryga.db").exists(),
        "既定値のSQLiteに書かれていません"
    );
}

/// 明示したファイルがあれば、その内容で動く。
#[test]
fn 明示した設定ファイルの内容で動く() {
    let dir = 作業場所();
    std::fs::write(
        dir.join("custom.toml"),
        "[database]\nurl = \"sqlite://custom.db?mode=rwc\"\n",
    )
    .unwrap();

    let output = dioryga(&dir)
        .args(["-c", "custom.toml", "migrate"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "失敗しました: {stderr}");
    assert!(
        dir.join("custom.db").exists(),
        "設定の接続先に書かれていません"
    );
    assert!(
        !dir.join("dioryga.db").exists(),
        "既定値のSQLiteに書かれています"
    );
}

//! 出力の途中で読み手が閉じても panic しないことの結合テスト（#193）。
//!
//! `dioryga licenses | less` で `less` を閉じると panic していた。単体テストは
//! 書き出し先を差し替えて確かめているが、**実際の標準出力とパイプで起きることを
//! 確かめるには、バイナリを起動するしかない。**

use std::io::Read;
use std::process::{Command, Stdio};

/// **読み手が途中で閉じても、panic せず正常に終わること。**
///
/// ライセンス表示は約290KBあり、パイプの緩衝（Linuxで64KB）に収まらない。
/// 冒頭だけ読んでパイプを閉じれば、残りを書こうとした時点で必ず読み手がいない。
#[test]
fn ライセンス表示の途中で読み手が閉じても正常に終わる() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dioryga"))
        .arg("licenses")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // 書き始めたことを確かめてから閉じる
    let mut stdout = child.stdout.take().unwrap();
    let mut 冒頭 = [0u8; 16];
    stdout.read_exact(&mut 冒頭).unwrap();
    drop(stdout);

    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "panic しています: {stderr}");
    assert!(
        output.status.success(),
        "正常に終わっていません（{}）: {stderr}",
        output.status
    );
}

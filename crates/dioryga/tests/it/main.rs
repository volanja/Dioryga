//! 結合テストの入口。
//!
//! **結合テストはすべてこの1本のバイナリにまとめる。**`tests/` 直下に
//! ファイルを置くと、cargoはそれぞれを別のバイナリとしてリンクする。1本ごとに
//! `dioryga` 全体をリンクし直すため、46本に分かれていた頃は `target/debug/deps`
//! が35GBを占め、リンク時間もテストの本数に比例して伸びていた。
//!
//! **テストを足すときは `tests/it/` にファイルを置き、下の一覧に `mod` を足す。**
//! 足し忘れると、そのファイルはコンパイルすらされない。[`すべてのファイルを読み込んでいる`]
//! がこれを検出する。

mod support;

mod account;
mod admin_cli;
mod admin_projects;
mod admin_users;
mod audit;
mod authz;
mod catalog;
mod catalog_import;
mod catalog_merge;
mod catalog_rest;
mod catalog_schema;
mod components;
mod costs;
mod costs_import;
mod dashboard;
mod db;
mod dev_autologin;
mod device_schema;
mod devices;
mod import_ui;
mod instance_import;
mod links;
mod login;
mod manifest_import;
mod members;
mod milestones;
mod navigation;
mod network;
mod network_import;
mod network_schema;
mod org_import;
mod parts_import;
mod placement_import;
mod placement_schema;
mod projects;
mod qcd_schema;
mod rack;
mod sbom;
mod session;
mod setup;
mod software_schema;
mod view;
mod warehouses;
mod work_order_schema;
mod work_orders;
mod workflow_import;

/// **`tests/it/` のファイルが、すべて上の一覧で読み込まれていること。**
#[test]
fn すべてのファイルを読み込んでいる() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it");
    let main = include_str!("main.rs");

    let mut 漏れ = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("tests/it を読めませんでした") {
        let path = entry.expect("tests/it を読めませんでした").path();
        let 名前 = match (path.is_dir(), path.file_stem()) {
            (true, Some(n)) if path.join("mod.rs").exists() => n,
            (false, Some(n)) if path.extension().is_some_and(|e| e == "rs") => n,
            _ => continue,
        };
        let 名前 = 名前.to_string_lossy();
        if 名前 != "main" && !main.lines().any(|l| l == format!("mod {名前};")) {
            漏れ.push(名前.into_owned());
        }
    }
    漏れ.sort();
    assert!(漏れ.is_empty(), "main.rs に mod がありません: {漏れ:?}");
}

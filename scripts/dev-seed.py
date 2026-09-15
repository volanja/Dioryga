#!/usr/bin/env python3
#
# 動作確認用データを入れて、開発用のサーバを起動する（#131）。
#
#   ./scripts/dev-seed.py                      DBを作り直してデータを入れる
#   ./scripts/dev-seed.py --serve              入れたあと、自動ログインで起動する
#   ./scripts/dev-seed.py --serve --as tateyama
#                                              別の利用者（Viewer）として画面を見る
#   ./scripts/dev-seed.py --keep               DBを作り直さずに流し直す
#                                              （2回流しても変わらないことの確認）
#   ./scripts/dev-seed.py --devices 5000       機器を生成して足す（多めのデータ）
#
# # 何をするか
#
# **取込を通して入れる。**開発用データだけの書き込み経路を作らない（設計書23.8）。
# 取込の権限の境界（3章）もそのまま通る。
#
#   1. 開発用のDB（.run/dev-seed.db）を作り直す。**普段の dioryga.db には触れない**
#   2. 最初の System Admin を作る（admin create --password-stdin）
#   3. 組織データを取り込む（System Admin）   docs/examples/organization/
#   4. カタログを取り込む（Operator）          scripts/dev-seed/catalog.yaml
#   5. プロジェクトごとに取り込む（編集者）    scripts/dev-seed/projects/*/
#   6. （--serve）自動ログインを有効にして起動する（dev-autologin 機能）
#
# # System Admin のパスワード
#
# **生成して .run/dev-seed-admin-password に置く**（.run は .gitignore 済み）。
# 画面を人が手で見るときに使う。自動ログインには要らない。
#
# 取込で作った利用者はパスワード未設定（23.8）。手でログインするときは
# `dioryga admin reset-password --username <名前>` で一時パスワードを発行する。
#
# # 入らないもの
#
# 取込の手段が無いものは入らない：什器（#139）、マイルストーン（#140）、
# 変更管理チケット（#141）、プロジェクトのアーカイブ（#142）。

from __future__ import annotations

import argparse
import os
import secrets
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SEED = REPO / "scripts" / "dev-seed"
ORGANIZATION = REPO / "docs" / "examples" / "organization" / "manifest.yaml"
RUN = REPO / ".run"

# 最初の System Admin。組織データの取込を流す
ADMIN = "kitadake"
ADMIN_NAME = "北岳 天守"

# カタログとプロジェクトの取込を流す利用者。**両プロジェクトの編集者**である
# （docs/examples/organization/members.csv）。System Admin はプロジェクトの
# データを取り込めない（3章）
IMPORTER = "hotaka"


def 実行(args: list[str], env: dict[str, str], stdin: str | None = None, check: bool = True):
    print("$", " ".join(str(a) for a in args), flush=True)
    result = subprocess.run(
        [str(a) for a in args], env=env, input=stdin, text=True, capture_output=True
    )
    if result.stdout.strip():
        print(result.stdout.rstrip())
    if result.returncode != 0 and check:
        print(result.stderr.rstrip(), file=sys.stderr)
        sys.exit(result.returncode)
    return result


def 取り込む(binary: Path, env: dict[str, str], manifest: Path, as_user: str) -> None:
    実行([binary, "import", manifest, "--as-user", as_user, "--apply"], env)


def 管理者を作る(binary: Path, env: dict[str, str]) -> None:
    password = secrets.token_urlsafe(18)
    result = 実行(
        [binary, "admin", "create", "--username", ADMIN, "--name", ADMIN_NAME, "--password-stdin"],
        env,
        stdin=password + "\n",
        check=False,
    )
    if result.returncode == 0:
        path = RUN / "dev-seed-admin-password"
        path.write_text(password + "\n", encoding="utf-8")
        path.chmod(0o600)
        print(f"System Admin「{ADMIN}」のパスワードを {path.relative_to(REPO)} に置きました")
    elif "既に存在" in result.stderr:
        print(f"System Admin「{ADMIN}」は既にいます（パスワードは前回のものです）")
    else:
        print(result.stderr.rstrip(), file=sys.stderr)
        sys.exit(result.returncode)


def 機器を生成する(count: int) -> Path:
    """カタクリ基盤更改に足す機器を生成する。**生成するのでファイルは持たない。**"""
    out = RUN / "dev-seed-large"
    out.mkdir(parents=True, exist_ok=True)
    rows = [
        "uid,external_id,hostname,serial_number,asset_number,device_type,"
        "configuration_vendor,configuration_model,configuration_name,power_watt,status"
    ]
    for i in range(1, count + 1):
        rows.append(
            f",GEN-{i:06d},katakuri-gen{i:05d},GEN{i:08d},,Physical,"
            f"ハイマツ電機,HM-2200,標準構成,450,running"
        )
    (out / "devices.csv").write_text("\n".join(rows) + "\n", encoding="utf-8")
    (out / "manifest.yaml").write_text(
        "format_version: 1\nkind: instances\nproject: \"KATAKURI-01\"\n"
        "match_on:\n  device: [serial_number]\n"
        "files:\n  - { entity: device, path: devices.csv }\n",
        encoding="utf-8",
    )
    return out / "manifest.yaml"


def main() -> int:
    p = argparse.ArgumentParser(description="動作確認用データを入れて、開発用のサーバを起動する")
    p.add_argument("--serve", action="store_true", help="入れたあと、自動ログインで起動する")
    p.add_argument("--as", dest="as_user", default=IMPORTER, help=f"自動ログインする利用者（既定 {IMPORTER}）")
    p.add_argument("--keep", action="store_true", help="DBを作り直さずに流し直す")
    p.add_argument("--devices", type=int, default=0, help="機器をこの数だけ生成して足す")
    p.add_argument("--port", type=int, default=8080, help="起動するポート（既定 8080）")
    args = p.parse_args()

    RUN.mkdir(exist_ok=True)
    db = RUN / "dev-seed.db"

    # 自動ログインの機能を立てて建てる。配布物のビルドには入らない（#128）
    subprocess.run(
        ["cargo", "build", "-q", "-p", "dioryga", "--features", "dev-autologin"],
        cwd=REPO,
        check=True,
    )
    binary = REPO / "target" / "debug" / ("dioryga.exe" if os.name == "nt" else "dioryga")

    if not args.keep:
        for suffix in ("", "-wal", "-shm"):
            Path(f"{db}{suffix}").unlink(missing_ok=True)

    env = os.environ.copy()
    env["DIORYGA_DATABASE__URL"] = f"sqlite://{db}?mode=rwc"
    env["DIORYGA_BIND"] = f"127.0.0.1:{args.port}"
    # 取込の間は自動ログインを効かせない（CLIには関係ないが、紛れを避ける）
    env.pop("DIORYGA_DEV_AUTOLOGIN", None)

    実行([binary, "migrate"], env)
    管理者を作る(binary, env)

    取り込む(binary, env, ORGANIZATION, ADMIN)
    取り込む(binary, env, SEED / "catalog.yaml", IMPORTER)
    for manifest in sorted((SEED / "projects").glob("*/manifest.yaml")):
        取り込む(binary, env, manifest, IMPORTER)
    if args.devices > 0:
        取り込む(binary, env, 機器を生成する(args.devices), IMPORTER)

    print()
    print(f"入れました：{db.relative_to(REPO)}")
    print("利用者：kitadake（System Admin）、yarigatake（Administrator）、hotaka（Administrator・Operator、2プロジェクト）、")
    print("        hakuba（Approver）、tateyama（Viewer）、yatsugatake（無効）")

    if not args.serve:
        return 0

    env["DIORYGA_DEV_AUTOLOGIN"] = args.as_user
    print(f"http://127.0.0.1:{args.port}/ を {args.as_user} として開けます（開発専用の自動ログイン）")
    print(flush=True)
    # サーバに置き換える。Ctrl-C がそのままサーバへ届く
    if os.name == "nt":
        return subprocess.run([str(binary)], env=env).returncode
    os.execve(str(binary), [str(binary)], env)
    return 0


if __name__ == "__main__":
    sys.exit(main())

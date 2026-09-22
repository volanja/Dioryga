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
#   6. マイルストーンとチケットを直に書く      ワークフローを入れる()
#   7. （--serve）自動ログインを有効にして起動する（dev-autologin 機能）
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
# 取込の手段が無いものは入らない：什器（#139）、プロジェクトのアーカイブ（#142）。
#
# マイルストーン（#140）と変更管理チケット（#141）も取込の手段が無いが、
# **ダッシュボードの4枠のうち2枠がこれで埋まる**ため、[`ワークフローを入れる`]
# でSQLiteへ直に書く。取込ができるようになったらその関数ごと捨てる。

from __future__ import annotations

import argparse
import os
import secrets
import sqlite3
import subprocess
import sys
from datetime import date, datetime, timedelta, timezone
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

# マイルストーンとチケットを載せるプロジェクト。**機器・契約が揃っているのは
# こちら**で、ダッシュボードの4枠が同時に埋まる
WORKFLOW_PROJECT = "KATAKURI-01"

# チケットの主担当（Operator）と承認者（Administrator Primary）。
# **別人にする。**同一人物だと自己承認になり、画面から起票した場合と違う形になる
ASSIGNEE = "hotaka"
APPROVER = "yarigatake"

# マイルストーン（#140）。予定日は**流した日からの差**で置く
#   (種別, 予定日の差, 状態, 説明)
MILESTONES = [
    ("ServiceStart", 10, "planned", "本番サービス開始"),
    ("ServiceUpdate", 40, "planned", "第2期ぶんの機器を載せる"),
    ("ServiceMaintenance", -15, "planned", "ファームウェアの定期更新"),
]

# 変更管理チケット（#141）。**5つの状態を1件ずつと、期限を過ぎたものを1件。**
# Repair の2件は `failed` / `repairing` の機器に紐づけてある——ダッシュボードの
# 「対応が必要な機器」がチケットへの導線を出す側と、起票を促す側の**両方**を見る
WORK_ORDERS = [
    {
        "status": "planned",
        "work_type": "Addition",
        "title": "katakuri-db01 をラックに載せる",
        "description": "第1期ぶんの最後の1台。搭載位置は未定。",
        "device": "katakuri-db01",
        "due": 30,
    },
    {
        "status": "planned",
        "work_type": "Addition",
        "title": "katakuri-app01 に25G NICを増設する",
        "description": "**期限を過ぎたまま planned で残っている。**",
        "device": "katakuri-app01",
        "due": -5,
    },
    {
        "status": "approved",
        "work_type": "Repair",
        "title": "katakuri-app02 のマザーボードを交換する",
        "description": "起動の途中で電源が落ちる。ベンダーの一次切り分け済み。",
        "device": "katakuri-app02",
        "due": 7,
    },
    {
        "status": "in_progress",
        "work_type": "Repair",
        "title": "katakuri-batch01 の電源ユニットを交換する",
        "description": "PSU2が冗長を失っている。部材は到着済み。",
        "device": "katakuri-batch01",
        "due": 3,
    },
    {
        "status": "completed",
        "work_type": "Relocation",
        "title": "katakuri-web02 を隣のラックへ移す",
        "description": "電源容量の偏りを均すため。",
        "device": "katakuri-web02",
        "due": -20,
    },
    {
        "status": "cancelled",
        "work_type": "Disposal",
        "title": "katakuri-vm01 を廃止する",
        "description": "移行先の見直しに伴い取りやめ。",
        "device": "katakuri-vm01",
        "due": -10,
        "cancelled_reason": "移行先が決まらず、当面は現行のまま運用する",
    },
]

# 承認が済んでいる状態。`approved` は導出結果であり、**承認行が真実の源**（11章）
承認済みの状態 = ("approved", "in_progress", "completed")


def ワークフローを入れる(db: Path) -> None:
    """マイルストーンと変更管理チケットを入れる（#140・#141）。

    **取込の経路が無いものだけ、ここでSQLiteへ直に書く。**23.8が禁じているのは
    製品に開発用の書き込み経路を足すことであり、この関数は製品のコードを1行も
    増やさない。取込ができるようになったら**この関数ごと捨てて** projects/*/ の
    CSVへ移す。

    **監査ログは残らない。**画面から起票したチケットと違うのはそこだけである。

    **日付は流すたびに今日から数え直す。**「10日後」「期限切れ」は確かめたい
    見え方そのものであり、固定日で書くと日が経つほど目的から外れる。
    """
    today = date.today()
    now = datetime.now(timezone.utc).isoformat()

    con = sqlite3.connect(db)
    con.execute("PRAGMA foreign_keys = ON")
    try:
        project = con.execute(
            "SELECT id FROM project WHERE code = ?", (WORKFLOW_PROJECT,)
        ).fetchone()[0]
        利用者 = {name: id for id, name in con.execute("SELECT id, username FROM app_user")}
        機器 = {name: id for id, name in con.execute("SELECT id, hostname FROM device")}

        # 流し直しても増やさない。**この関数が入れた行だけ**を消してから入れる。
        # 画面から手で作ったチケットには触れない
        題名 = [w["title"] for w in WORK_ORDERS]
        枠 = ",".join("?" * len(題名))
        con.execute(
            "DELETE FROM work_order_approval WHERE work_order_id IN"
            f" (SELECT id FROM work_order WHERE project_id = ? AND title IN ({枠}))",
            [project, *題名],
        )
        con.execute(
            f"DELETE FROM work_order WHERE project_id = ? AND title IN ({枠})",
            [project, *題名],
        )
        説明 = [m[3] for m in MILESTONES]
        枠 = ",".join("?" * len(説明))
        con.execute(
            f"DELETE FROM milestone WHERE project_id = ? AND description IN ({枠})",
            [project, *説明],
        )

        for milestone_type, 差, status, description in MILESTONES:
            con.execute(
                "INSERT INTO milestone (project_id, milestone_type, planned_date,"
                " actual_date, status, description, created_at, updated_at)"
                " VALUES (?, ?, ?, NULL, ?, ?, ?, ?)",
                (
                    project,
                    milestone_type,
                    (today + timedelta(days=差)).isoformat(),
                    status,
                    description,
                    now,
                    now,
                ),
            )

        for w in WORK_ORDERS:
            status = w["status"]
            着手した = status in ("in_progress", "completed")
            row = con.execute(
                "INSERT INTO work_order (project_id, target_project_id, device_id,"
                " part_instance_id, work_type, title, description,"
                " primary_assignee_id, secondary_assignee_id, due_date, status,"
                " planned_at, executed_at, completed_at, cancelled_at,"
                " cancelled_reason, created_at, updated_at)"
                " VALUES (?, NULL, ?, NULL, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    project,
                    機器[w["device"]],
                    w["work_type"],
                    w["title"],
                    w["description"],
                    利用者[ASSIGNEE],
                    (today + timedelta(days=w["due"])).isoformat(),
                    status,
                    now,
                    now if 着手した else None,
                    now if status == "completed" else None,
                    now if status == "cancelled" else None,
                    w.get("cancelled_reason"),
                    now,
                    now,
                ),
            )
            承認済み = status in 承認済みの状態
            con.execute(
                "INSERT INTO work_order_approval (work_order_id, required_project_id,"
                " approver_id, status, approved_at, self_approved, created_at, updated_at)"
                " VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
                (
                    row.lastrowid,
                    project,
                    利用者[APPROVER] if 承認済み else None,
                    "approved" if 承認済み else "pending",
                    now if 承認済み else None,
                    now,
                    now,
                ),
            )
        con.commit()
    finally:
        con.close()

    print(
        f"マイルストーン{len(MILESTONES)}件・変更管理チケット{len(WORK_ORDERS)}件を"
        f"{WORKFLOW_PROJECT}に入れました（取込の経路が無いため直に書いています）"
    )


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

    ワークフローを入れる(db)

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

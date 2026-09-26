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
#   6. マイルストーンとチケットを取り込む      期限のあるデータを作る()
#   7. （--serve）自動ログインを有効にして起動する（dev-autologin 機能）
#
# # パスワード
#
# **System Admin のパスワードは生成して admin create に渡すだけで、どこにも
# 残さない**（#197）。取込で作った利用者はパスワード未設定（23.8）。
#
# 画面を見るだけなら自動ログインで足りる。System Admin の画面も同じ：
#
#   ./scripts/dev-seed.py --serve --keep --as kitadake
#
# 手でログインするときは、一時パスワードを発行する（次のログインで変更を求められる）：
#
#   DIORYGA_DATABASE__URL='sqlite://.run/dev-seed.db' \
#     cargo run --features dev-autologin -- admin reset-password --username kitadake
#
# # 入らないもの
#
# 取込の手段が無いものは入らない：プロジェクトのアーカイブ（#142）。
#
# 什器と搭載は取込から入る（#139）。katakuri-db01 は載せない——増設のチケット
# 「katakuri-db01 をラックに載せる」が、搭載位置は未定のまま残っている。
#
# **マイルストーンと変更管理チケットは取込から入る**（#140・#141）。以前は
# 取込の経路が無く、SQLiteへ直に書いていた。**その関数は消えた。**

from __future__ import annotations

import argparse
import os
import secrets
import subprocess
import sys
from datetime import date, timedelta
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


def 期限のあるデータを作る() -> Path:
    """マイルストーンとチケットのCSVを書き出す（#140・#141）。**ファイルは持たない。**

    **日付が流した日から決まる**ため、固定のCSVにできない。「10日後」「期限切れ」は
    確かめたい見え方そのもので、固定日で書くと日が経つほど目的から外れる。
    機器を生成する場合（`--devices`）と同じ扱いにしてある。

    **承認行は状態から決まる。**進んでいるチケット（approved 以降）は承認済み、
    それ以外は承認待ちにする。揃っていないと取込が警告を出す（11.4-7）。
    """
    out = RUN / "dev-seed-workflow"
    out.mkdir(parents=True, exist_ok=True)
    today = date.today()

    def 日付(差: int) -> str:
        return (today + timedelta(days=差)).isoformat()

    マイルストーン = ["uid,external_id,milestone_type,planned_date,actual_date,status,description"]
    for i, (種別, 差, status, 説明) in enumerate(MILESTONES, start=1):
        マイルストーン.append(f",MS-{i:03d},{種別},{日付(差)},,{status},{説明}")
    (out / "milestones.csv").write_text("\n".join(マイルストーン) + "\n", encoding="utf-8")

    チケット = [
        "uid,external_id,work_type,title,description,device_hostname,"
        "primary_assignee,secondary_assignee,due_date,status,cancelled_reason"
    ]
    承認 = ["work_order,required_project,approver,status"]
    for i, w in enumerate(WORK_ORDERS, start=1):
        番号 = f"WO-{i:03d}"
        チケット.append(
            f",{番号},{w['work_type']},{w['title']},{w['description']},{w['device']},"
            f"{ASSIGNEE},,{日付(w['due'])},{w['status']},{w.get('cancelled_reason', '')}"
        )
        承認済み = w["status"] in 承認済みの状態
        承認.append(
            f"{番号},{WORKFLOW_PROJECT},{APPROVER if 承認済み else ''},"
            f"{'approved' if 承認済み else 'pending'}"
        )
    (out / "work-orders.csv").write_text("\n".join(チケット) + "\n", encoding="utf-8")
    (out / "approvals.csv").write_text("\n".join(承認) + "\n", encoding="utf-8")

    (out / "manifest.yaml").write_text(
        "format_version: 1\nkind: instances\n"
        f'project: "{WORKFLOW_PROJECT}"\n'
        "files:\n"
        "  - { entity: milestone,            path: milestones.csv }\n"
        "  - { entity: work_order,           path: work-orders.csv }\n"
        "  - { entity: work_order_approval,  path: approvals.csv }\n",
        encoding="utf-8",
    )
    return out / "manifest.yaml"


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
    # **パスワードはどこにも残さない**（#197）。手でログインするときは
    # reset-password で一時パスワードを発行する（冒頭の説明）
    if result.returncode == 0:
        print(f"System Admin「{ADMIN}」を作りました")
    elif "既に存在" in result.stderr:
        print(f"System Admin「{ADMIN}」は既にいます")
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

    # **期限は流した日から決まる。**生成して取込を通す（機器の生成と同じ扱い）
    取り込む(binary, env, 期限のあるデータを作る(), IMPORTER)

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

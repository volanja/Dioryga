#!/usr/bin/env python3
#
# ビルド成果物（target/）から、**今のビルドが参照していない世代**を消す。
#
#   ./scripts/clean-target.py            点検のみ。何がどれだけ消えるかを出す
#   ./scripts/clean-target.py --apply    実際に消す
#   ./scripts/clean-target.py --apply --unknown-profiles
#                                        定義の無いプロファイルの出力ごと消す
#
# # なぜ `cargo clean` ではないのか
#
# **`cargo clean` は全部消す。**次のビルドが依存クレートを含めた全再構築になり、
# 待ち時間が大きい。ここで消したいのは「積もった旧世代」だけで、現在の世代は
# 残したい。
#
# **cargoは古い世代を回収しない。**同じターゲットでもコンパイル条件が変わると
# `名前-<16桁のハッシュ>` の別世代が作られ、以前の世代はそのまま残る。実測では
# `dioryga` が118世代あり、`target/` が105GBまで膨らんでいた。
#
# # 何を「使っていない」とみなすか
#
# **ターゲット名ごとに、最新の世代だけを残す。**cargoが参照するのは現在の
# コンパイル条件に対応する世代だけで、それ以外は過去のソース状態の産物である。
#
# **直近に更新されたものは、最新でなくても残す**（既定24時間、`--keep-hours`）。
# `cargo build` と `cargo test` ではプロファイルや機能フラグが異なり、**同時に
# 2つ以上の世代が現役でありうる**ため、mtimeだけで切ると現役を消してしまう。
#
# `incremental/` も同じ考え方で、クレートごとに最新のセッションだけを残す。
#
# # 消しても壊れない
#
# **`target/` に消えて困るものは無い。**必要になればcargoが作り直す。代償は
# 再ビルドの時間だけで、判定を誤っても失われるデータは無い。
#
# 既定を点検のみにしているのは、**何がどれだけ消えるかを見てから実行できる
# ようにする**ため（設計書23.6のドライランと同じ考え方）。

from __future__ import annotations

import argparse
import os
import re
import shutil
import sys
import time

# `名前-<16桁のハッシュ>` と、それに続く拡張子。`lib` 接頭辞は名前の一部として扱う
成果物 = re.compile(r"^(lib)?([A-Za-z0-9_]+)-([0-9a-f]{16})(\..*)?$")

# cargoが既定で使うプロファイルの出力先。`dev`/`test` は `debug` に出る
既定の出力先 = {"debug", "release"}

# プロファイルの出力ではないもの
対象外 = {"tmp", "package", "doc", "CACHEDIR.TAG", ".rustc_info.json"}


def 容量(path: str) -> int:
    total = 0
    for root, _, files in os.walk(path):
        for f in files:
            try:
                total += os.lstat(os.path.join(root, f)).st_size
            except OSError:
                pass
    return total


def 読みやすく(size: int) -> str:
    for 単位 in ("B", "KiB", "MiB", "GiB"):
        if size < 1024 or 単位 == "GiB":
            return f"{size:.1f} {単位}" if 単位 != "B" else f"{size} B"
        size /= 1024.0
    return f"{size:.1f} GiB"


class 集計:
    def __init__(self) -> None:
        self.files = 0
        self.size = 0

    def 足す(self, files: int, size: int) -> None:
        self.files += files
        self.size += size


def プロファイル出力を探す(target: str) -> list[str]:
    """`deps/` と `.fingerprint/` を持つディレクトリを、プロファイルの出力とみなす。

    `target/debug` だけでなく、`cargo-llvm-cov` のように独自の `--target-dir` を
    使う場合の入れ子（`target/llvm-cov-target/debug`）も拾う。**名前で決め打ち
    しない**のは、増えたときに拾い漏らさないため。
    """
    out = []
    for root, dirs, _ in os.walk(target):
        if os.path.isdir(os.path.join(root, "deps")) and os.path.isdir(
            os.path.join(root, ".fingerprint")
        ):
            out.append(root)
            # この下をさらに掘っても、プロファイルの入れ子は無い
            dirs[:] = []
    return sorted(out)


def 定義済みプロファイル(repo: str) -> set[str]:
    """`Cargo.toml` の `[profile.X]` と、cargoの既定を合わせて返す。"""
    names = set(既定の出力先)
    try:
        with open(os.path.join(repo, "Cargo.toml"), encoding="utf-8") as f:
            for line in f:
                m = re.match(r"^\[profile\.([A-Za-z0-9_-]+)\]", line.strip())
                if m:
                    names.add(m.group(1))
    except OSError:
        pass
    return names


def 旧世代を消す(profile: str, keep_seconds: float, apply: bool, now: float) -> 集計:
    """`deps/` から、ターゲット名ごとの最新以外の世代を消す。"""
    deps = os.path.join(profile, "deps")
    世代: dict[tuple[str, str], dict] = {}

    for name in os.listdir(deps):
        m = 成果物.match(name)
        if not m:
            continue
        ターゲット = (m.group(1) or "") + m.group(2)
        path = os.path.join(deps, name)
        try:
            st = os.lstat(path)
        except OSError:
            continue
        e = 世代.setdefault((ターゲット, m.group(3)), {"files": [], "size": 0, "mtime": 0.0})
        e["files"].append(path)
        e["size"] += st.st_size
        e["mtime"] = max(e["mtime"], st.st_mtime)

    最新: dict[str, str] = {}
    for (ターゲット, ハッシュ), e in 世代.items():
        現 = 最新.get(ターゲット)
        if 現 is None or e["mtime"] > 世代[(ターゲット, 現)]["mtime"]:
            最新[ターゲット] = ハッシュ

    結果 = 集計()
    for (ターゲット, ハッシュ), e in 世代.items():
        if ハッシュ == 最新[ターゲット] or now - e["mtime"] < keep_seconds:
            continue
        結果.足す(len(e["files"]), e["size"])
        if apply:
            for p in e["files"]:
                try:
                    os.remove(p)
                except OSError:
                    pass
    return 結果


def 古いセッションを消す(profile: str, keep_seconds: float, apply: bool, now: float) -> 集計:
    """`incremental/` から、クレートごとの最新以外のセッションと旧世代を消す。"""
    inc = os.path.join(profile, "incremental")
    結果 = 集計()
    if not os.path.isdir(inc):
        return 結果

    クレート別: dict[str, list[tuple[float, str]]] = {}
    for name in os.listdir(inc):
        path = os.path.join(inc, name)
        if not os.path.isdir(path):
            continue
        クレート別.setdefault(name.rsplit("-", 1)[0], []).append(
            (os.lstat(path).st_mtime, path)
        )

        # 同じディレクトリの中の、古いセッション
        sessions = []
        for s in os.listdir(path):
            sp = os.path.join(path, s)
            if s.startswith("s-") and os.path.isdir(sp):
                sessions.append((os.lstat(sp).st_mtime, sp))
        sessions.sort()
        for _, sp in sessions[:-1]:
            結果.足す(1, 容量(sp))
            if apply:
                shutil.rmtree(sp, ignore_errors=True)

    # 同じクレートの旧世代ディレクトリ
    for _, dirs in クレート別.items():
        dirs.sort()
        for mtime, path in dirs[:-1]:
            if now - mtime < keep_seconds or not os.path.isdir(path):
                continue
            結果.足す(1, 容量(path))
            if apply:
                shutil.rmtree(path, ignore_errors=True)
    return 結果


def 定義の無い出力を探す(target: str, 定義済み: set[str]) -> list[str]:
    """`Cargo.toml` に定義が無いプロファイルの出力先を返す。

    プロファイルを消したり、名前を変えたりすると、**出力だけが残る。**実際に
    `target/slim` が1.5GB残っていた（定義は一度もコミットされていない）。
    """
    out = []
    for name in sorted(os.listdir(target)):
        path = os.path.join(target, name)
        if name in 対象外 or not os.path.isdir(path):
            continue
        # 入れ子（`llvm-cov-target/debug` 等）は、その中で判断する
        if not os.path.isdir(os.path.join(path, "deps")):
            continue
        if name not in 定義済み:
            out.append(path)
    return out


def main() -> int:
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    p = argparse.ArgumentParser(
        description="target/ から、今のビルドが参照していない世代を消す"
    )
    p.add_argument("--apply", action="store_true", help="実際に消す（既定は点検のみ）")
    p.add_argument(
        "--keep-hours",
        type=float,
        default=24.0,
        help="最新でなくても、この時間内に更新された世代は残す（既定24）",
    )
    p.add_argument(
        "--unknown-profiles",
        action="store_true",
        help="Cargo.toml に定義の無いプロファイルの出力を、まるごと消す",
    )
    p.add_argument("--target", default=os.path.join(repo, "target"), help="target ディレクトリ")
    args = p.parse_args()

    target = os.path.abspath(args.target)
    if not os.path.isdir(target):
        print(f"{target} がありません", file=sys.stderr)
        return 1
    # **target の外は絶対に触らない。**判定を誤ってもソースへ及ばないようにする
    if os.path.basename(target) != "target":
        print(f"target という名前のディレクトリを指してください: {target}", file=sys.stderr)
        return 1

    now = time.time()
    keep = args.keep_hours * 3600
    合計 = 集計()

    for profile in プロファイル出力を探す(target):
        a = 旧世代を消す(profile, keep, args.apply, now)
        b = 古いセッションを消す(profile, keep, args.apply, now)
        合計.足す(a.files + b.files, a.size + b.size)
        表示 = os.path.relpath(profile, os.path.dirname(target))
        print(f"{表示}: 旧世代 {読みやすく(a.size)} / incremental {読みやすく(b.size)}")

    定義の無い = 定義の無い出力を探す(target, 定義済みプロファイル(repo))
    for path in 定義の無い:
        size = 容量(path)
        表示 = os.path.relpath(path, os.path.dirname(target))
        if args.unknown_profiles and args.apply:
            shutil.rmtree(path, ignore_errors=True)
            合計.足す(1, size)
            print(f"{表示}: 定義の無いプロファイル {読みやすく(size)} を削除")
        else:
            print(
                f"{表示}: 定義の無いプロファイル {読みやすく(size)}"
                "（--unknown-profiles を付けると消します）"
            )

    if 合計.size:
        動作 = "消しました" if args.apply else "消せます（--apply を付けると実行）"
        print()
        print(f"合計 {読みやすく(合計.size)} を{動作}")
    else:
        print()
        print("消せるものはありません")
    return 0


if __name__ == "__main__":
    sys.exit(main())

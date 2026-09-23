#!/usr/bin/env python3
#
# ビルド成果物（target/）から、**今のビルドが参照していない世代**を消す。
#
#   ./scripts/clean-target.py            点検のみ。何がどれだけ消えるかを出す
#   ./scripts/clean-target.py --apply    実際に消す
#   ./scripts/clean-target.py --apply --unknown-profiles
#                                        定義の無いプロファイルの出力ごと消す
#   ./scripts/clean-target.py --keep-hours 24
#                                        cargoを走らせず、更新時刻で判定する
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
# # 何を「使っている」とみなすか
#
# **cargo自身に答えさせる。**[`現役のコマンド`] を `--message-format=json` 付きで
# 走らせると、cargoは更新の要らなかったユニットについても `compiler-artifact` を
# 出力し、そこに `deps/<名前>-<ハッシュ>` が入る。**これに現れない世代は、
# どのコマンドからも参照されていない。**ソースの無くなったターゲット（統合前の
# 旧テストや、手元のスクラッチ）もこれで消える。
#
# 以前は「ターゲット名ごとに最新＋直近24時間」で判定していたが、**開発中は
# ほぼすべてが24時間以内に入り、何も消えなかった**（66GBがすべて当日の世代）。
# 窓を縮めても、その間に作業を重ねれば同じことになる。この方式は `--keep-hours`
# で残してある（cargoを走らせられないとき用）。
#
# **ソースが前回のビルドより新しいと、ここでビルドが走る。**ハッシュを得るには
# cargoにビルド計画を立てさせるしかなく、計画だけを出す安定版の手段が無い。
# 出力先がまだ無いコマンドは走らせない（点検のために新しい世代を作らない）。
#
# # 消しても壊れない
#
# **`target/` に消えて困るものは無い。**必要になればcargoが作り直す。代償は
# 再ビルドの時間だけで、判定を誤っても失われるデータは無い。[`現役のコマンド`]
# に無い組み合わせ（例：`--release` の clippy）は、次に使うとき作り直しになる。
#
# 既定を点検のみにしているのは、**何がどれだけ消えるかを見てから実行できる
# ようにする**ため（設計書23.6のドライランと同じ考え方）。

from __future__ import annotations

import argparse
import fcntl
import filecmp
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field

# `名前-<16桁のハッシュ>` と、それに続く拡張子。`lib` 接頭辞は名前の一部として扱う
成果物 = re.compile(r"^(lib)?([A-Za-z0-9_]+)-([0-9a-f]{16})(\..*)?$")

# cargoが既定で使うプロファイルの出力先。`dev`/`test` は `debug` に出る
既定の出力先 = {"debug", "release"}

# プロファイルの出力ではないもの
対象外 = {"tmp", "package", "doc", "CACHEDIR.TAG", ".rustc_info.json"}

# 「使っている」とみなすコマンド。(出力先, cargoの引数, llvm-covの環境で走らせるか)
#
# **CIと同じ組み合わせと、機能フラグ無しの日常の組み合わせを両方持つ。**
# `dev-autologin` の有無で `dioryga` 以下のハッシュが変わり、手元では両方が
# 現役になっている。
現役のコマンド: list[tuple[str, list[str], bool]] = [
    ("debug", ["build", "--workspace"], False),
    ("debug", ["test", "--workspace", "--no-run"], False),
    ("debug", ["test", "--workspace", "--no-run", "--features", "dioryga/dev-autologin"], False),
    ("debug", ["clippy", "--workspace", "--all-targets"], False),
    ("debug", ["clippy", "--workspace", "--all-targets", "--features", "dioryga/dev-autologin"], False),
    ("release", ["build", "--workspace", "--release"], False),
    ("ci", ["build", "--workspace", "--profile", "ci"], False),
    # `cargo llvm-cov` 自体は `--message-format` を受け付けない。`show-env` の環境と
    # 同じ出力先で `cargo test --no-run` を走らせると、同じハッシュになる
    (
        "llvm-cov-target/debug",
        ["test", "--workspace", "--no-run", "--features", "dioryga/dev-autologin"],
        True,
    ),
]


def 容量(path: str) -> int:
    """ディスク上で実際に占めている量。

    `st_size` ではなくブロック数で数える。APFSの圧縮やクローンがあると
    `st_size` の合計は `du` よりかなり大きく出る（44GiB と出て、実際に空いたのは
    24GB だった）。
    """
    try:
        st = os.lstat(path)
    except OSError:
        return 0
    if not os.path.isdir(path):
        return st.st_blocks * 512
    total = 0
    for root, _, files in os.walk(path):
        for f in files:
            try:
                total += os.lstat(os.path.join(root, f)).st_blocks * 512
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


def 消す(path: str, apply: bool, 結果: 集計) -> None:
    結果.足す(1, 容量(path))
    if not apply:
        return
    if os.path.isdir(path) and not os.path.islink(path):
        shutil.rmtree(path, ignore_errors=True)
    else:
        try:
            os.remove(path)
        except OSError:
            pass


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


# ---------------------------------------------------------------------------
# 既定：cargoの出力から現役を決める
# ---------------------------------------------------------------------------


@dataclass
class 現役:
    """1つのプロファイル出力について、cargoが参照していると答えたもの。"""

    # `(名前, ハッシュ)`。`lib` 接頭辞は外す——lib の `.d` は `名前-<ハッシュ>.d` になる
    世代: set[tuple[str, str]] = field(default_factory=set)
    # ワークスペースのクレートごとの、incremental を持つユニットの数
    ユニット数: dict[str, int] = field(default_factory=dict)
    # 数え済みのユニット（同じユニットが複数のコマンドに現れる）
    数えた: set[str] = field(default_factory=set)


def llvm_covの環境() -> dict[str, str] | None:
    """`cargo llvm-cov show-env` の出力を環境変数にする。無ければ None。"""
    try:
        r = subprocess.run(
            ["cargo", "llvm-cov", "show-env"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    env = {}
    for line in r.stdout.splitlines():
        for 語 in shlex.split(line):
            if "=" in 語:
                k, v = 語.split("=", 1)
                env[k] = v
    return env


def 同じ実体を探す(profile: str, path: str) -> str | None:
    """`target/debug/dioryga` のように deps の外へ出された成果物について、
    中身が同じ deps 側の名前を返す。

    **bin の `filenames` は deps ではなく、出し先のパスで報告される。**これを
    見落とすと、使っている bin の世代を消してしまう。

    **inode では突き合わせられない。**macOSではcargoがハードリンクでなく複製
    （APFSのクローン）で出すため、inode が別になる。名前と大きさで候補を絞り、
    中身を比べる。
    """
    name = os.path.basename(path)
    base, dot, ext = name.partition(".")
    候補 = re.compile(rf"^{re.escape(base)}-[0-9a-f]{{16}}{re.escape(dot + ext)}$")
    deps = os.path.join(profile, "deps")
    try:
        size = os.lstat(path).st_size
    except OSError:
        return None
    for d in os.listdir(deps):
        dp = os.path.join(deps, d)
        try:
            if 候補.match(d) and os.lstat(dp).st_size == size and filecmp.cmp(
                path, dp, shallow=False
            ):
                return d
        except OSError:
            continue
    return None


def 現役を集める(
    repo: str, target: str, 出力先: set[str]
) -> tuple[dict[str, 現役], set[str]] | None:
    """[`現役のコマンド`] を走らせ、プロファイル出力ごとの現役を返す。

    2つ目の戻り値は、コマンドを走らせたプロファイル出力の集合。**失敗したら
    None を返す**——集合が欠けたまま消すと、現役を消してしまう。
    """
    結果: dict[str, 現役] = {}
    走らせた: set[str] = set()
    cov_env: dict[str, str] | None = None

    for 相対, 引数, llvm_cov in 現役のコマンド:
        profile = os.path.join(target, 相対)
        if profile not in 出力先:
            # 出力先がまだ無い。点検のために新しい世代を作らない
            continue

        env = dict(os.environ)
        if llvm_cov:
            if cov_env is None:
                cov_env = llvm_covの環境() or {}
            if not cov_env:
                print(f"{相対}: cargo-llvm-cov が無いため触りません", file=sys.stderr)
                continue
            env.update(cov_env)
            env["CARGO_TARGET_DIR"] = os.path.dirname(profile)

        cmd = ["cargo", *引数, "--locked", "--message-format=json"]
        print(f"$ {shlex.join(cmd)}", file=sys.stderr)
        # 進み具合（Compiling ...）はそのまま見せる。JSONは stdout に出る
        r = subprocess.run(cmd, cwd=repo, env=env, stdout=subprocess.PIPE, text=True)
        if r.returncode != 0:
            print(
                f"cargo が失敗しました（終了コード {r.returncode}）。何も消しません",
                file=sys.stderr,
            )
            return None
        走らせた.add(profile)
        現 = 結果.setdefault(profile, 現役())

        for line in r.stdout.splitlines():
            try:
                m = json.loads(line)
            except json.JSONDecodeError:
                continue
            if m.get("reason") != "compiler-artifact":
                continue

            paths = list(m.get("filenames") or [])
            if m.get("executable"):
                paths.append(m["executable"])
            名前一覧 = set()
            for p in paths:
                name = os.path.basename(p)
                if os.path.basename(os.path.dirname(p)) != "deps":
                    name = 同じ実体を探す(profile, p) or ""
                a = 成果物.match(name)
                if a:
                    現.世代.add((a.group(2), a.group(3)))
                    名前一覧.add(name)

            # incremental はワークスペースのクレートだけが持つ
            if str(m.get("package_id", "")).startswith("path+"):
                クレート = m["target"]["name"].replace("-", "_")
                キー = f"{m['package_id']} {m['target']['kind']} {sorted(名前一覧)}"
                if キー not in 現.数えた:
                    現.数えた.add(キー)
                    現.ユニット数[クレート] = 現.ユニット数.get(クレート, 0) + 1

    return 結果, 走らせた


def 使っていない世代を消す(profile: str, 現: 現役, 開始: float, apply: bool) -> 集計:
    """`deps/` から、cargoが参照していない世代を消す。"""
    deps = os.path.join(profile, "deps")
    結果 = 集計()
    for name in os.listdir(deps):
        m = 成果物.match(name)
        if not m or (m.group(2), m.group(3)) in 現.世代:
            continue
        path = os.path.join(deps, name)
        try:
            # **走らせている間に別のセッションが作ったものは残す**
            if os.lstat(path).st_mtime >= 開始:
                continue
        except OSError:
            continue
        消す(path, apply, 結果)
    return 結果


def 生成単位(path: str) -> set[str]:
    """incremental のディレクトリが持つ、コード生成単位の名前。

    セッションの中の `<単位>.o` は、deps の `<名前>-<ハッシュ>.<単位>.<乱数>.rcgu.o`
    と同じ名前を持つ。**これでディレクトリと世代を結び付けられる。**
    """
    out = set()
    for s in os.listdir(path):
        sp = os.path.join(path, s)
        if s.startswith("s-") and os.path.isdir(sp):
            out.update(f[:-2] for f in os.listdir(sp) if f.endswith(".o"))
    return out


def 使っていないincrementalを消す(
    profile: str, 現: 現役, 開始: float, apply: bool
) -> 集計:
    """`incremental/` から、現役のユニットに対応しないディレクトリを消す。

    ディレクトリ名のハッシュは成果物のハッシュと別物で、名前からは対応が取れない。
    そこで、
    1. **コード生成したもの**は、deps の `.o` と生成単位の名前で結び付ける
    2. それ以外（clippy の検査だけのもの等）は、クレートごとに現役のユニット数
       まで、新しいものから残す
    """
    inc = os.path.join(profile, "incremental")
    結果 = 集計()
    if not os.path.isdir(inc):
        return 結果

    現役の単位 = set()
    for name in os.listdir(os.path.join(profile, "deps")):
        m = 成果物.match(name)
        if m and name.endswith(".rcgu.o") and (m.group(2), m.group(3)) in 現.世代:
            現役の単位.add(name.split(".")[1])

    クレート別: dict[str, list[tuple[float, str]]] = {}
    for name in os.listdir(inc):
        path = os.path.join(inc, name)
        if os.path.isdir(path):
            クレート別.setdefault(name.rsplit("-", 1)[0], []).append(
                (os.lstat(path).st_mtime, path)
            )

    for クレート, dirs in クレート別.items():
        残り = 現.ユニット数.get(クレート, 0)
        結び付いた = [d for d in dirs if 生成単位(d[1]) & 現役の単位]
        残す = {p for _, p in 結び付いた}
        for mtime, path in sorted(dirs, reverse=True):
            if path in 残す:
                continue
            if mtime >= 開始 or len(残す) < 残り:
                残す.add(path)
                continue
            消す(path, apply, 結果)

        for path in 残す:
            古いセッションを消す(path, apply, 結果)
    return 結果


def 古いセッションを消す(path: str, apply: bool, 結果: 集計) -> None:
    """同じディレクトリの中で、最新以外のセッションを消す。"""
    sessions = []
    for s in os.listdir(path):
        sp = os.path.join(path, s)
        if s.startswith("s-") and os.path.isdir(sp):
            sessions.append((os.lstat(sp).st_mtime, sp))
    sessions.sort()
    for _, sp in sessions[:-1]:
        消す(sp, apply, 結果)


class cargoのロック:
    """プロファイル出力を、cargoと同じロックで押さえる。

    **このリポジトリは複数のセッションが同時にビルドする。**消している最中に
    別のセッションがビルドすると、作りかけの世代を消しうる。cargoはビルドの間
    `<プロファイル>/.cargo-lock` を flock で押さえるので、同じロックを取る。
    """

    def __init__(self, profile: str) -> None:
        self.path = os.path.join(profile, ".cargo-lock")
        self.f = None

    def __enter__(self) -> None:
        self.f = open(self.path, "a")
        try:
            fcntl.flock(self.f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print(f"{self.path} の解放を待っています（ビルド中）", file=sys.stderr)
            fcntl.flock(self.f, fcntl.LOCK_EX)

    def __exit__(self, *_: object) -> None:
        if self.f:
            fcntl.flock(self.f, fcntl.LOCK_UN)
            self.f.close()


# ---------------------------------------------------------------------------
# --keep-hours：cargoを走らせず、更新時刻で判定する
# ---------------------------------------------------------------------------


def 旧世代を時刻で消す(profile: str, keep_seconds: float, apply: bool, now: float) -> 集計:
    """`deps/` から、ターゲット名ごとの最新以外の世代を消す。

    **判定は粗い。**ソースの無くなったターゲットは最新の世代が残り続け、逆に
    同じ名前で複数の世代が現役のもの（lib と bin、機能フラグの有無）は、窓の外に
    出た時点で現役を消す。cargoを走らせられないとき（ソースがコンパイルできない
    等）の代替として残している。
    """
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
        e = 世代.setdefault((ターゲット, m.group(3)), {"files": [], "mtime": 0.0})
        e["files"].append(path)
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
        for p in e["files"]:
            消す(p, apply, 結果)
    return 結果


def incrementalを時刻で消す(profile: str, keep_seconds: float, apply: bool, now: float) -> 集計:
    """`incremental/` から、クレートごとの最新以外のディレクトリと古いセッションを消す。"""
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
        古いセッションを消す(path, apply, 結果)

    for _, dirs in クレート別.items():
        dirs.sort()
        for mtime, path in dirs[:-1]:
            if now - mtime < keep_seconds or not os.path.isdir(path):
                continue
            消す(path, apply, 結果)
    return 結果


# ---------------------------------------------------------------------------


def main() -> int:
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    p = argparse.ArgumentParser(
        description="target/ から、今のビルドが参照していない世代を消す"
    )
    p.add_argument("--apply", action="store_true", help="実際に消す（既定は点検のみ）")
    p.add_argument(
        "--keep-hours",
        type=float,
        help="cargoを走らせず、ターゲット名ごとの最新と、この時間内に更新された世代を残す",
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

    合計 = 集計()
    出力先 = プロファイル出力を探す(target)

    if args.keep_hours is not None:
        now = time.time()
        keep = args.keep_hours * 3600
        for profile in 出力先:
            with cargoのロック(profile):
                a = 旧世代を時刻で消す(profile, keep, args.apply, now)
                b = incrementalを時刻で消す(profile, keep, args.apply, now)
            合計.足す(a.files + b.files, a.size + b.size)
            表示 = os.path.relpath(profile, os.path.dirname(target))
            print(f"{表示}: 旧世代 {読みやすく(a.size)} / incremental {読みやすく(b.size)}")
    else:
        開始 = time.time()
        集めた = 現役を集める(repo, target, set(出力先))
        if 集めた is None:
            return 1
        現役一覧, 走らせた = 集めた
        print(file=sys.stderr)
        for profile in 出力先:
            表示 = os.path.relpath(profile, os.path.dirname(target))
            if profile not in 走らせた:
                print(f"{表示}: 対応するコマンドが無いため触りません")
                continue
            with cargoのロック(profile):
                a = 使っていない世代を消す(profile, 現役一覧[profile], 開始, args.apply)
                b = 使っていないincrementalを消す(profile, 現役一覧[profile], 開始, args.apply)
            合計.足す(a.files + b.files, a.size + b.size)
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

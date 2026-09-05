#!/usr/bin/env bash
#
# 依存ライセンスの表示（THIRD-PARTY-NOTICES.txt）を生成する。
#
#   ./scripts/licenses.sh          生成して更新する
#   ./scripts/licenses.sh --check  生成せず、既存の出力が最新かだけを確かめる
#
# **単一バイナリで配布する以上、依存が求める著作権表示とライセンス全文の保持を
# バイナリ側で満たす必要がある。**生成物は `include_str!` で埋め込まれ、
# `dioryga licenses` で出力される。
#
# 設定は about.toml、書式は about.hbs。**新しいライセンスが依存に入ると
# about.toml の accepted に無いため生成が失敗する**——これが検査として働く。
#
# 必要なもの：cargo-about
#
#   cargo install cargo-about --locked --features cli
set -euo pipefail

cd "$(dirname "$0")/.."

OUT=crates/dioryga/THIRD-PARTY-NOTICES.txt

command -v cargo-about >/dev/null || {
  echo "cargo-about が見つかりません。次で入れてください：" >&2
  echo "  cargo install cargo-about --locked --features cli" >&2
  exit 1
}

if [ "${1:-}" = "--check" ]; then
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT

  echo "依存ライセンスの表示を生成して突き合わせます"
  cargo about generate about.hbs -o "$tmp"

  if diff -u "$OUT" "$tmp"; then
    echo "$OUT は最新です"
  else
    echo "" >&2
    echo "$OUT が古くなっています。ローカルで ./scripts/licenses.sh を実行してコミットしてください。" >&2
    exit 1
  fi
else
  echo "依存ライセンスの表示を生成します"
  cargo about generate about.hbs -o "$OUT"
  echo "$OUT を更新しました"
fi

#!/usr/bin/env bash
#
# スキーマドキュメント（docs/schema/）を生成する。
#
#   ./scripts/schema-docs.sh          生成して docs/schema/ を更新する
#   ./scripts/schema-docs.sh --check  生成せず、既存の出力が最新かだけを確かめる
#
# **手書きの定義からではなく、マイグレーションを適用した実DBから生成する**
# （設計書2章）。使い捨てのPostgreSQLコンテナを立て、終わったら必ず落とす。
#
# 必要なもの：Docker（macOSはColima。DOCKER_HOSTの設定は dev-environment.md 参照）、tbls
set -euo pipefail

cd "$(dirname "$0")/.."

# **名前にプロセス番号を入れる。**固定の名前だと、同時に走らせた2つ目が
# 1つ目のコンテナを消してしまう（#177）。
CONTAINER="dioryga-schema-docs-$$"
# **版の出所は .postgres-version ひとつ。**テスト（crates/dioryga/tests/it/support/mod.rs）
# も同じファイルを読む。同じ版を2箇所に書くと必ず食い違い、スキーマの検査と
# 動作の検証が別のPostgreSQLに対して行われることになる（#76）。
PG_IMAGE="postgres:$(tr -d '[:space:]' < .postgres-version)"

for cmd in docker tbls cargo; do
  command -v "$cmd" >/dev/null || { echo "$cmd が見つかりません" >&2; exit 1; }
done

# 途中で失敗してもコンテナを残さない。強制終了で残ったものは名前もポートも
# 重ならないので、次の実行を妨げない（`docker ps -a` で見て消す）。
cleanup() { docker rm -f "$CONTAINER" >/dev/null 2>&1 || true; }
trap cleanup EXIT

# **ホスト側のポートは既定でDockerに割り当てさせる**（#177）。固定ポートは、
# 他のものが使っていると変更の中身と無関係に落ちる。起動前に空きを探す方式は、
# 探してから使うまでの間に取られうるので採らない。
# SCHEMA_DOCS_PORT を与えれば、そのポートを使う。
echo "PostgreSQLを起動します（使い捨て、$PG_IMAGE）"
docker run -d --name "$CONTAINER" \
  -e POSTGRES_PASSWORD=postgres \
  -p "${SCHEMA_DOCS_PORT:+${SCHEMA_DOCS_PORT}:}5432" \
  "$PG_IMAGE" >/dev/null

# 割り当てられたポートを読む。`docker port` は `0.0.0.0:49153` と `[::]:49153`
# のように1行ずつ出すので、最初の行の最後の `:` より後ろを取る。
# （パイプで head に渡さないのは、pipefail の下で SIGPIPE を拾わないため）
binding="$(docker port "$CONTAINER" 5432/tcp)"
binding="${binding%%$'\n'*}"
PORT="${binding##*:}"
case "$PORT" in
  '' | *[!0-9]*)
    echo "割り当てられたポートを読めませんでした: ${binding:-（出力なし）}" >&2
    exit 1
    ;;
esac
# **ポートはここで1度だけ決め、tblsとマイグレーションの両方へ渡す**
export TBLS_DSN="postgres://postgres:postgres@127.0.0.1:${PORT}/postgres?sslmode=disable"

# 起動を待つ。固定のsleepにしないのは、遅い環境で不安定になるため。
#
# **待ち切れなかった理由を必ず出す。**出さないと、CIでは「exit 2」しか
# 残らず、コンテナが落ちたのかポートが埋まっていたのか判別できない。
ready=false
for _ in $(seq 1 120); do
  if docker exec "$CONTAINER" pg_isready -U postgres >/dev/null 2>&1; then
    ready=true
    break
  fi
  # コンテナ自体が落ちているなら、待っても無駄
  if [ "$(docker inspect -f '{{.State.Running}}' "$CONTAINER" 2>/dev/null)" != "true" ]; then
    break
  fi
  sleep 1
done

if [ "$ready" != "true" ]; then
  echo "PostgreSQLが起動しませんでした。コンテナの状態とログを出します。" >&2
  docker ps -a --filter "name=$CONTAINER" >&2 || true
  docker logs "$CONTAINER" >&2 2>&1 || true
  exit 1
fi

echo "マイグレーションを適用します"
DIORYGA_DATABASE__URL="postgres://postgres:postgres@127.0.0.1:${PORT}/postgres" \
  cargo run --quiet -- migrate

echo "スキーマを検査します"
# 外部キー索引の張り忘れとテーブルコメントの欠落を検出する
tbls lint -c .tbls.yml

if [ "${1:-}" = "--check" ]; then
  echo "ドキュメントが最新かを確認します"
  # 差分があれば非ゼロで終わる
  tbls diff -c .tbls.yml
  echo "docs/schema/ は最新です"
else
  echo "ドキュメントを生成します"
  tbls doc -c .tbls.yml --rm-dist
  echo "docs/schema/ を更新しました"
fi

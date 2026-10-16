#!/usr/bin/env bash
# corpus-refresh.sh — 语料仓滚动刷新 (ci-periodic 载体: weekly 保新鲜, 防 fixture 腐坏)
#
# 职责边界: 语料仓 (luochen1990/log-skimmer-corpus) GHA 侧槽位滚动重拉 + 主仓 master
#   引擎重算压缩率基线 + 单 commit 推回。语料与基线**原子配对提交** — 主仓 PR 判定
#   用的基线恒为 "master 引擎 × 当前语料", 无失配窗口。
# 边界:
#   - Forgejo 侧 10 对快照不自动刷新 (刷新执行器可达该实例, 保留 manual 补抓: 本脚本
#     跳过 git.lambda.lc__* 目录; 手动补抓走 fetch-logs 家族, 语料仓直接 commit)。
#   - 新语料若让 master 引擎 FN>0 (新失败形态规则未覆盖), 本脚本不阻断 (基线只含
#     压缩率) — 该信号由主仓下一个 PR 的 l0-gate 红暴露, 归因明确, 属新鲜语料的
#     设计价值 (倒逼规则集持续覆盖最新形态)。
# 凭据: 推送语料仓需 env CORPUS_GH_TOKEN (GitHub PAT, contents write; CI secret 注入)。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${CORPUS_DIR:-$ROOT/data/logs}"
WANT_FAIL=14
WANT_SUCCESS=5

log() { printf '[corpus-refresh %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

[ -n "${CORPUS_GH_TOKEN:-}" ] || { echo "缺 CORPUS_GH_TOKEN (推送语料仓用)" >&2; exit 1; }

# 1. 语料仓工作副本在场 (ci-periodic 前置 fetch-corpus.sh 已建; 本地手动跑亦然)
[ -d "$DEST/.git" ] || { echo "语料仓工作副本不存在: $DEST (先跑 fetch-corpus.sh)" >&2; exit 1; }

# 2. GHA 侧槽位滚动: 清旧拉新 (fetch-logs 幂等配额: 每仓 14F+5S)
rm -rf "$DEST"/github.com__*
WANT_FAIL=$WANT_FAIL WANT_SUCCESS=$WANT_SUCCESS CORPUS_DIR="$DEST" bash "$ROOT/scripts/fetch-logs.sh"

# 3. master 引擎重算基线 (collect → gate --update-baseline 写入语料仓内)
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/log-skimmer"
[ -x "$BIN" ] || { echo "skim 二进制不存在: $BIN (先 cargo build --release)" >&2; exit 1; }
CORPUS_DIR="$DEST" LOG_SKIMMER_BIN="$BIN" bash "$ROOT/scripts/l0-collect.sh" >/dev/null
CORPUS_DIR="$DEST" bash "$ROOT/scripts/l0-gate.sh" --update-baseline

# 4. 单 commit 推回 (语料+基线原子配对)
cd "$DEST"
git config user.name "corpus-bot"
git config user.email "corpus-bot@users.noreply.github.com"
git add -A
if git diff --cached --quiet; then
  log "语料无变化, 无提交"
else
  git commit -qm "corpus refresh $(date -u +%Y-%m-%dT%H:%MZ)"
  git push --quiet "https://x-access-token:${CORPUS_GH_TOKEN}@github.com/luochen1990/log-skimmer-corpus.git" master
  log "已刷新并推送: $(git log --oneline -1)"
fi

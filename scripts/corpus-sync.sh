#!/usr/bin/env bash
# corpus-sync.sh — 判定器镜像单向同步 (ci-periodic 载体; 替代已退役的内网刷新)
#
# 职责边界: 主仓判定器 (engine 源码 + 规则 + L0 门禁脚本) → 语料仓镜像目录,
#   有差则推送并 dispatch 触发语料仓 GHA 即时刷新 (新引擎 × 当前语料重算基线,
#   消除基线失配窗口)。语料仓 refresh.yml 在 GitHub 机房自刷 (槽位重拉+基线重算),
#   本脚本只送文本镜像 (~150KB 级, 低频), 语料字节不再走内网出口 (省外网流量)。
# SSOT: 主仓是判定器唯一编辑源; 语料仓 Cargo.toml/src/rules/scripts 是只读镜像,
#   禁止在语料仓手编 (下次同步即被覆盖)。
# 依赖: bash + git + curl; env CORPUS_GH_TOKEN (推送 + dispatch, contents write)。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${CORPUS_DIR:-$ROOT/data/logs}"
GH_REPO="luochen1990/log-skimmer-corpus"

log() { printf '[corpus-sync %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

[ -n "${CORPUS_GH_TOKEN:-}" ] || { echo "缺 CORPUS_GH_TOKEN (推送语料仓 + dispatch 用)" >&2; exit 1; }

# 1. 语料仓工作副本在场/更新 (幂等 clone/pull; 内网现场语料 fetch-forgejo 由
#    ci-periodic 另步保证 — 本脚本只管镜像, 不动语料数据)
bash "$ROOT/scripts/fetch-corpus.sh"
[ -d "$DEST/.git" ] || { echo "语料仓工作副本不在场: $DEST" >&2; exit 1; }

# 2. 组装镜像 (engine 提升为语料仓根包 — 脱离主仓 workspace 独立可构建)
cd "$DEST"
rm -rf src rules scripts
rm -f Cargo.toml
cp "$ROOT/engine/Cargo.toml" Cargo.toml
cp -r "$ROOT/engine/src" src
cp -r "$ROOT/rules" rules
mkdir scripts
cp "$ROOT/scripts/l0-collect.sh" "$ROOT/scripts/l0-summarize.jq" \
   "$ROOT/scripts/l0-gate.sh" "$ROOT/scripts/fetch-logs.sh" scripts/

# 3. 有差则推送 + dispatch 即时刷新 (基线跟上新引擎)
git config user.name "corpus-sync"
git config user.email "corpus-sync@users.noreply.github.com"
git add -A
if git diff --cached --quiet; then
  log "镜像无变化 (master 判定器与语料仓一致)"
else
  SRC_SHA=$(git -C "$ROOT" rev-parse --short HEAD)
  git commit -qm "engine sync ${SRC_SHA} (主仓判定器镜像 — 勿手编, 下次同步覆盖)"
  git push --quiet "https://x-access-token:${CORPUS_GH_TOKEN}@github.com/${GH_REPO}.git" master
  log "镜像已推送: $(git log --oneline -1)"
  curl -sfL --max-time 30 -X POST \
    -H "Authorization: Bearer ${CORPUS_GH_TOKEN}" -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/${GH_REPO}/dispatches" \
    -d '{"event_type":"engine-sync"}'
  log "已 dispatch engine-sync (语料仓将即时重算基线)"
fi

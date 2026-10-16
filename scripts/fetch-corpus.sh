#!/usr/bin/env bash
# fetch-corpus.sh — 语料仓获取/更新 (幂等, L0 门禁语料前置)
#
# 职责边界: 把 GitHub 语料仓 (luochen1990/log-skimmer-corpus, public) 克隆/更新到
#   data/logs (语料+基线恒配对版本化 — 压缩率防退化判定的配对保证)。
#   语料是互联网可拉取的公开 CI 日志滚动快照, 不入主仓 (评论裁决 2026-10-16:
#   入仓语料只会腐坏; 独立语料仓由主仓 ci-periodic 持续刷新保新鲜)。
# 幂等/离线容忍:
#   - data/logs 已是语料仓 clone → git pull --ff-only; 网络失败时若本地有 67 对
#     槽位样本则降级 warn 用现有副本 (开发者离线可跑门禁); CORPUS_OFFLINE=1 跳过拉取。
#   - CI 全新 workspace → clone 必须 succeed (语料不在场由 l0-gate 的 ≥60 断言兜底红)。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${CORPUS_DIR:-$ROOT/data/logs}"
URL="https://github.com/luochen1990/log-skimmer-corpus.git"
MIN_SLOTS=60 # 与 l0-gate 的语料在场断言同源 (67 对留余量)

log() { printf '[fetch-corpus %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

count_samples() { find "$DEST" -name '*.log.json' 2>/dev/null | wc -l; }

if [ "${CORPUS_OFFLINE:-0}" = 1 ]; then
  log "CORPUS_OFFLINE=1, 跳过拉取"
  exit 0
fi

if [ -d "$DEST/.git" ]; then
  if git -C "$DEST" pull --ff-only --quiet 2>/dev/null; then
    log "已更新 ($(count_samples) 对样本)"
  elif [ "$(count_samples)" -ge "$MIN_SLOTS" ]; then
    log "WARN: pull 失败, 降级用本地副本 ($(count_samples) 对)"
  else
    log "pull 失败且本地样本不足"; exit 1
  fi
else
  mkdir -p "$(dirname "$DEST")"
  git clone --depth 1 --quiet "$URL" "$DEST"
  log "已克隆 ($(count_samples) 对样本)"
fi

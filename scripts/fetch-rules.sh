#!/usr/bin/env bash
# fetch-rules.sh — 规则集获取/更新 (幂等, L0 门禁规则前置)
#
# 职责边界: 把规则仓 (github.com/luochen1990/log-skimmer-rules, public, 社区维护)
#   克隆/更新到 data/rules。规则是引擎运行时的单向依赖 (独立成仓, 需求方裁决
#   2026-10-16 四仓拓扑), 本仓不携带规则文件 — dev/CI 一律经此获取。
# 幂等/离线容忍:
#   - data/rules 已是规则仓 clone → git pull --ff-only; 网络失败时若本地 ≥25 条
#     规则则降级 warn 用现有副本; RULES_OFFLINE=1 跳过拉取。
#   - CI 全新 workspace → clone 必须 succeed (规则不在场由 l0-gate 断言兜底红)。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${RULES_DIR:-$ROOT/data/rules}"
URL="https://github.com/luochen1990/log-skimmer-rules.git"
MIN_RULES=25 # 30 条现状留余量, 与 rules 仓 lint 的在场断言同源

log() { printf '[fetch-rules %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

# ! -path '.github': 规则仓模板已迁 .github/ 隐藏路径 (issue #8), 引擎装载跳过;
#   计数同步排除整个 .github/ — 防未来该目录出现其他 yaml 误计数 (与 rules 仓 lint.yml 同款加固)
count_rules() { find "$DEST" -name '*.yaml' ! -name '_template.yaml' ! -path '*/.github/*' 2>/dev/null | wc -l; }

if [ "${RULES_OFFLINE:-0}" = 1 ]; then
  log "RULES_OFFLINE=1, 跳过拉取"
  exit 0
fi

if [ -d "$DEST/.git" ]; then
  if git -C "$DEST" pull --ff-only --quiet 2>/dev/null; then
    log "已更新 ($(count_rules) 条规则)"
  elif [ "$(count_rules)" -ge "$MIN_RULES" ]; then
    log "WARN: pull 失败, 降级用本地副本 ($(count_rules) 条)"
  else
    log "pull 失败且本地规则不足"; exit 1
  fi
else
  mkdir -p "$(dirname "$DEST")"
  git clone --depth 1 --quiet "$URL" "$DEST"
  log "已克隆 ($(count_rules) 条规则)"
fi

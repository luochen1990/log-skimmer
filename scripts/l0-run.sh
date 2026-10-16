#!/usr/bin/env bash
# l0-run.sh — L0 门禁编排 (薄; 判定口径与断言 SSOT 在 engine/src/l0.rs)
#
# 职责边界: 构建 PR 引擎 → 检出并构建 base 引擎 (origin/master, 离线降级单测)
#   → base 实测 → PR 双测判定。just l0-gate 与 CI 同链调用本脚本。
# base 侧兼容: master 可能尚无 `l0` 子命令 (本 PR 引入) — 探测: 有则直接用;
#   无则回退 base worktree 自带的 bash 采集链 (l0-collect.sh + l0-summarize.jq,
#   master 判定器自洽形态 — 引擎+脚本同源), 语义同为 "master 判定器 × 当前语料规则"。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build --release -q
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/log-skimmer" # CI 持久卷缓存重定向
CORPUS="${CORPUS_DIR:-$ROOT/data/logs}" # 同上: CI 语料缓存卷
RULES="${RULES_DIR:-$ROOT/data/rules}" 

if BASE_BIN=$(bash scripts/l0-base.sh 2>/dev/null); then
  WT=/tmp/opencode/l0-base-wt
  if "$BASE_BIN" l0 --help >/dev/null 2>&1; then
    "$BASE_BIN" l0 --corpus "$CORPUS" --rules "$RULES" --emit-summary > /tmp/l0-s-base.json
  elif [ -f "$WT/scripts/l0-collect.sh" ]; then
    # 中间形态 master (有 bash 采集链无 l0 子命令的窗口期): 用其自带链
    RULES_DIR="$RULES" CORPUS_DIR="$CORPUS" LOG_SKIMMER_BIN="$BASE_BIN" \
      bash "$WT/scripts/l0-collect.sh" | jq -s -f "$WT/scripts/l0-summarize.jq" > /tmp/l0-s-base.json
  else
    # 奠基 PR 场景: master (POC 形态) 无可比判定链 — 降级单测; 合并后 master
    # 具备完整链, 后续 PR 双测自动生效
    echo "WARN: master 无 l0 子命令亦无 bash 采集链 (POC 形态), 降级单测" >&2
    "$BIN" l0 --corpus "$CORPUS" --rules "$RULES"
    exit $?
  fi
  "$BIN" l0 --corpus "$CORPUS" --rules "$RULES" --base /tmp/l0-s-base.json
else
  echo "WARN: base 引擎不可得 (离线?), 降级单测 (压缩率对比跳过)" >&2
  "$BIN" l0 --corpus "$CORPUS" --rules "$RULES"
fi

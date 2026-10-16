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
BIN="$ROOT/target/release/log-skimmer"

if BASE_BIN=$(bash scripts/l0-base.sh 2>/dev/null); then
  WT=/tmp/opencode/l0-base-wt
  if "$BASE_BIN" l0 --help >/dev/null 2>&1; then
    "$BASE_BIN" l0 --corpus data/logs --rules data/rules --emit-summary > /tmp/l0-s-base.json
  else
    RULES_DIR="$ROOT/data/rules" CORPUS_DIR="$ROOT/data/logs" LOG_SKIMMER_BIN="$BASE_BIN" \
      bash "$WT/scripts/l0-collect.sh" | jq -s -f "$WT/scripts/l0-summarize.jq" > /tmp/l0-s-base.json
  fi
  "$BIN" l0 --corpus data/logs --rules data/rules --base /tmp/l0-s-base.json
else
  echo "WARN: base 引擎不可得 (离线?), 降级单测 (压缩率对比跳过)" >&2
  "$BIN" l0 --corpus data/logs --rules data/rules
fi

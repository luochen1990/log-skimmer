#!/usr/bin/env bash
# l0-run.sh — L0 门禁编排 (薄; 判定口径与断言 SSOT 在 engine/src/l0.rs)
#
# 职责边界: 构建 PR 引擎 → 构建并实测 base 引擎 (origin/master, 离线降级单测)
#   → 双测判定。just l0-gate 与 CI 同链调用本脚本。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build --release -q
BIN="$ROOT/target/release/log-skimmer"

if BASE_BIN=$(bash scripts/l0-base.sh 2>/dev/null); then
  "$BASE_BIN" l0 --corpus data/logs --rules data/rules --emit-summary > /tmp/l0-s-base.json
  "$BIN" l0 --corpus data/logs --rules data/rules --base /tmp/l0-s-base.json
else
  echo "WARN: base 引擎不可得 (离线?), 降级单测 (压缩率对比跳过)" >&2
  "$BIN" l0 --corpus data/logs --rules data/rules
fi

#!/usr/bin/env bash
# l0-run.sh — L0 门禁编排 (薄; 判定口径与断言 SSOT 在 engine/src/l0.rs)
#
# 职责边界: 构建 PR 引擎 → 检出并构建 base 引擎 (origin/master, 离线降级单测)
#   → base 实测 → PR 双测判定。just l0-gate 与 CI 同链调用本脚本。
# base 侧兼容: master 可能尚无 `l0` 子命令 (本 PR 引入) — 探测: 有则直接用;
#   无则回退 base worktree 自带的 bash 采集链 (l0-collect.sh + l0-summarize.jq,
#   master 判定器自洽形态 — 引擎+脚本同源), 语义同为 "master 判定器 × 当前语料规则"。
# 降级哨兵 (issue #112, 降级不红但醒目可见 — 不红是 #36 的 DNS 容错初衷):
#   - 降级路径 stderr 恒带 "⚠ DUAL-TEST DEGRADED" 前缀 (CI 日志 grep 点);
#   - 正常双测完成路径末行输出成功标记 "dual-test: ok (base=<rev>)",
#     标记缺席本身即异常信号 (base 不可得/构建失败/门禁红都不会有标记)。
#   - base worktree/summary 路径带 pid 后缀 (防同 runner 并发 job 互踩),
#     本脚本是这些路径的唯一 Owner (内联赋值会静默覆盖外部导出的同名 env,
#     属有意 — 共享场景经下方 L0_BASE_WT 显式传递), trap 统一清理。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p /tmp/opencode # SUM 重定向的目录依赖显式化 (勿依赖 worktree add 的副作用)

WT="/tmp/opencode/l0-base-wt.$$" # pid 后缀防并发互踩; 经 L0_BASE_WT 与 l0-base.sh 共享
SUM="/tmp/opencode/l0-s-base.$$.json"
trap 'rm -rf "$WT" "$SUM"; git -C "$ROOT" worktree prune >/dev/null 2>&1 || :' EXIT
trap 'exit 1' INT TERM # 非交互 bash 的 EXIT trap 不拦截信号致死, 转发以触发清理
degraded() { echo "⚠ DUAL-TEST DEGRADED: $*" >&2; }

cargo build --release -q
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/log-skimmer" # CI 持久卷缓存重定向
CORPUS="${CORPUS_DIR:-$ROOT/data/logs}" # 同上: CI 语料缓存卷
RULES="${RULES_DIR:-$ROOT/data/rules}"

if BASE_BIN=$(L0_BASE_WT="$WT" bash scripts/l0-base.sh); then
  BASE_REV="$(git -C "$WT" rev-parse --short HEAD)"
  if "$BASE_BIN" l0 --help >/dev/null 2>&1; then
    "$BASE_BIN" l0 --corpus "$CORPUS" --rules "$RULES" --emit-summary > "$SUM"
  elif [ -f "$WT/scripts/l0-collect.sh" ]; then
    # 中间形态 master (有 bash 采集链无 l0 子命令的窗口期): 用其自带链
    RULES_DIR="$RULES" CORPUS_DIR="$CORPUS" LOG_SKIMMER_BIN="$BASE_BIN" \
      bash "$WT/scripts/l0-collect.sh" | jq -s -f "$WT/scripts/l0-summarize.jq" > "$SUM"
  else
    # 奠基 PR 场景: master (POC 形态) 无可比判定链 — 降级单测; 合并后 master
    # 具备完整链, 后续 PR 双测自动生效
    degraded "master 无 l0 子命令亦无 bash 采集链 (POC 形态), 降级单测"
    "$BIN" l0 --corpus "$CORPUS" --rules "$RULES"
    exit $?
  fi
  "$BIN" l0 --corpus "$CORPUS" --rules "$RULES" --base "$SUM"
  echo "dual-test: ok (base=$BASE_REV)" # 成功标记: 缺席即异常 (见头注释)
else
  degraded "base 引擎不可得 (离线/检出/构建失败), 降级单测 (压缩率对比跳过)"
  "$BIN" l0 --corpus "$CORPUS" --rules "$RULES"
fi

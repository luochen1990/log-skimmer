#!/usr/bin/env bash
# l0-base.sh — base 引擎构建 (L0 双测编排的 base 侧)
#
# 职责边界: 检出 base ref (默认 origin/master) 到临时 worktree 并构建 release 二进制,
#   stdout 输出二进制路径 — 供 l0-gate 双测的第二轮 collect 用 (master 引擎 × 当前
#   语料/规则)。同语料同规则下, master vs PR 两轮实测的压缩率 diff 即纯引擎变量。
# 缓存: 共享 CARGO_TARGET_DIR (CI 持久卷 / 本地 target) — base 与 PR 代码差异小,
#   cargo 增量交替编译代价低。
# 退化场景 (跑在 master 上, base=自身): diff 恒 0, 双测退化为单测 — 语义无损。
# worktree 路径 (issue #112): 默认带 pid 后缀防同 runner 并发 job 互踩; 编排方
#   (l0-run.sh) 经 L0_BASE_WT 传共享路径并负责清理 (worktree 须存活到 l0-run.sh
#   消费完 base 采集链, 本脚本不自杀清理); 独立运行的泄漏件由宿主 /tmp 回收。
# 降级哨兵 (issue #112): 降级不红但醒目可见 — "DUAL-TEST DEGRADED" 前缀供 CI
#   日志 grep, 与 l0-run.sh 降级哨兵同族。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BASE_REF="${L0_BASE_REF:-origin/master}"
WT="${L0_BASE_WT:-/tmp/opencode/l0-base-wt.$$}"
# rm 守卫 (review #113 批): WT 可被 env 覆盖为任意路径, 无条件 rm -rf 会删掉
#   覆盖值指向的真实目录 — 仅对 /tmp/opencode/ 下路径执行清理, 其余路径交给
#   worktree prune / 宿主回收。
case "$WT" in /tmp/opencode/*) rm -rf "$WT" ;; *) : ;; esac

log() { printf '[l0-base %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

git -C "$ROOT" fetch --quiet origin master 2>/dev/null || \
  { [ "$BASE_REF" = "master" ] || log "⚠ DUAL-TEST DEGRADED: fetch origin 失败 (离线?), base 用本地 ${BASE_REF}"; }

git -C "$ROOT" worktree prune
git -C "$ROOT" worktree add --detach --quiet "$WT" "$BASE_REF" 2>/dev/null || {
  log "WARN: base 检出失败 (ref ${BASE_REF} 不可得?), 退出非零供编排降级单测"
  exit 1
}
log "base ref: $(git -C "$WT" rev-parse --short HEAD) ($(git -C "$WT" log -1 --format=%s | head -c 60))"

# 独立 -base 旁挂 target (CI 持久卷同卷旁挂 / 本地 target-base): 共享 target 会被
# base 构建覆盖同名 bin (PR bin 变 master 版), 双测两引擎必须物理隔离产物
BASE_TARGET="${CARGO_TARGET_DIR:-$ROOT/target}-base"
CARGO_TARGET_DIR="$BASE_TARGET" cargo build --release --quiet --manifest-path "$WT/engine/Cargo.toml"
BIN="$BASE_TARGET/release/log-skimmer"
[ -x "$BIN" ] || { echo "base 引擎构建失败: $BIN 不存在" >&2; exit 1; }
echo "$BIN"

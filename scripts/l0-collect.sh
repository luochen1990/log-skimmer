#!/usr/bin/env bash
# 职责: L0 数据采集 SSOT — 单遍跑全语料 skim, 每样本一行 JSONL 输出到 stdout。
# l0-gate.sh (门禁判定) 与 l0-stats.sh (人类报表) 都消费本脚本产出, 口径单点驻留:
#   - rerun-through (attempt>1 且最终 success) 标记 excluded=true, 消费端滤出主指标;
#   - 语料当前无 cancelled/skipped 样本 (spec §4 三态处理暂无对象; 出现时在本地单点补)。
# 门禁化决策: skim 调用无兜底 — exit 2 (规则集/日志非法) 直接 fail;
#   exit 3 (零块, spec §3 结构承诺失明信号) 记 zero_block=1 正常输出 (违例判定在 gate)。
# BIN 定位 (优先级): $LOG_SKIMMER_BIN > $CARGO_TARGET_DIR/release/ > <repo>/target/release/
#   (workspace 根 target; CI 持久卷经 CARGO_TARGET_DIR 自动命中)。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RL="$ROOT/rules"
DL="${CORPUS_DIR:-$ROOT/data/logs}" # 语料根 (fetch-corpus.sh 克隆的语料仓工作副本; 参数化供 periodic 刷新复用)
BIN="${LOG_SKIMMER_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/release/log-skimmer}"
[ -x "$BIN" ] || { echo "skim 二进制不存在: $BIN (先 cargo build --release)" >&2; exit 1; }

for j in $(find "$DL" -name '*.log.json' | sort); do # 递归遍历 (语料嵌套布局 host/owner/repo/<date>/)
  log="${j%.log.json}.log"; [ -f "$log" ] || continue
  con=$(jq -r .conclusion "$j"); att=$(jq -r '.attempt // 1' "$j"); repo=$(jq -r .repo "$j")
  sample="${log#"$DL"/}"
  set +e
  r=$("$BIN" skim "$log" --rules "$RL" --json 2>/dev/null)
  rc=$?
  set -e
  # rc: 0 正常 / 3 零块 (记 zero_block 后继续) / 其余 fail-fast (门禁无兜底)
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 3 ]; then
    echo "skim 失败 (exit $rc): $sample" >&2
    exit 1
  fi
  jq -c --arg repo "$repo" --arg con "$con" --argjson att "$att" --arg sample "$sample" --arg ci "$(jq -r '.ci // ""' "$j")" '
    {
      repo: $repo, conclusion: $con, attempt: $att, sample: $sample, ci: $ci,
      excluded: ($att > 1 and $con == "success"),
      input_lines: .input_lines,
      output_lines: .output_lines,
      signature_output_lines: .stats.signature_output_lines,
      failure_frags: [.blocks[].fragments[] | select(.intent == "failure")] | length,
      failure_frag_lines: [.blocks[].fragments[] | select(.intent == "failure")
        | (.block[1] - .block[0] + 1)] | add // 0,
      warning_frags: [.blocks[].fragments[] | select(.intent == "warning")] | length,
      info_frags: [.blocks[].fragments[] | select(.intent == "info")] | length,
      zero_block: (if (.blocks | length) == 0 then 1 else 0 end),
      boundary_violations: (.stats.boundary_violations | length),
      frags: [.blocks[].fragments[]
        | {rule, intent, lines: (.block[1] - .block[0] + 1)}]
    }' <<<"$r"
done

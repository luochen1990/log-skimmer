#!/usr/bin/env bash
# 职责: L0 人类可读报表 — 消费 l0-collect.sh JSONL (统计口径 SSOT: scripts/l0-summarize.jq),
# 渲染 per repo×conclusion 汇总 / per-rule 分列 / rg 基线对照 三张表。
# 演进注记: POC 期主循环内联 skim 调用带 `|| r='{...}'` 兜底 (兼容写法), 门禁化 (M1) 后
#   采集职责移 l0-collect.sh 且 fail-fast (skim 失败直接红), 本脚本不再接触 skim 二进制;
#   BIN 定位亦随之退役 (原 engine/target/ 路径已因 workspace 化失效)。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DL="$ROOT/data/logs"

JSONL=$(bash "$ROOT/scripts/l0-collect.sh")
SUM=$(jq -s -f "$ROOT/scripts/l0-summarize.jq" <<<"$JSONL")

echo "=== 汇总 (per repo × conclusion; rerun-through 已排除) ==="
printf '%-40s %-9s %5s %10s %10s %4s %8s %16s\n' repo concl runs 'comp%blk' 'comp%sig' FN 'FP(fi)' 'warn/info(succ)'
jq -r '.per_repo_conclusion | to_entries | sort_by(.key) | .[] |
  (.key | split("|")) as [$repo, $con] |
  [$repo, $con, .value.runs,
   (.value.comp_blk_pct * 100 | round / 100), (.value.comp_sig_pct * 100 | round / 100),
   .value.fn, .value.fp_frags, .value.success_warning_frags, .value.success_info_frags]
  | @tsv' <<<"$SUM" \
| while IFS=$'\t' read -r repo con runs cb cs fn fp wn inf; do
    if [ "$con" = failure ]; then
      printf '%-40s %-9s %5s %9s%% %9s%% %4s %8s %16s\n' "$repo" "$con" "$runs" "$cb" "$cs" "$fn" "$fp" "-"
    else
      printf '%-40s %-9s %5s %9s %9s %4s %8s %16s\n' "$repo" "$con" "$runs" "-" "-" "-" "$fp" "${wn}w/${inf}i"
    fi
  done
echo

echo "=== per-rule (全语料主指标; FP=failure-intent 在 success job 的 fragment 数; repos=失败侧命中 repo 数) ==="
printf '%-44s %6s %7s %5s %6s\n' rule frags lines FP repos
jq -r '.per_rule | to_entries | sort_by(-.value.frags) | .[] |
  [.key, .value.frags, .value.lines, .value.fp_frags, .value.hit_repos] | @tsv' <<<"$SUM" \
| while IFS=$'\t' read -r rule frags lines fp repos; do
    printf '%-44s %6s %7s %5s %6s\n' "$rule" "$frags" "$lines" "$fp" "$repos"
  done
echo

# --- rg 基线对照 (error|warn 匹配行 ∪ tail-100, 粗基线; 与 skim 无关的独立对照实验) ---
BIN_=0; BOUT=0; BFP=0; BFN=0; BN=0; BSC=0
while IFS=$'\t' read -r sample con excluded in_; do
  [ "$excluded" = true ] && continue
  log="$DL/$sample"
  bl=$( { grep -ciE 'error|warn' "$log" || true; tail -100 "$log" | wc -l; } | awk 'NR==1{a=$1} NR==2{b=$1} END{print (a>b)?a:b}')
  BIN_=$((BIN_+in_)); BOUT=$((BOUT+bl)); BN=$((BN+1))
  if [ "$con" = success ]; then
    BSC=$((BSC+1)); BFP=$((BFP + $(grep -ciE 'error|warn' "$log" || true)))
  else
    [ "$bl" -eq 0 ] && BFN=$((BFN+1))
  fi
done < <(jq -r '[.sample, .conclusion, .excluded, .input_lines] | @tsv' <<<"$JSONL")

echo "=== rg 基线 (error|warn ∪ tail-100) vs Log Skimmer ==="
bc=$((10000*(BIN_-BOUT)/(BIN_>0?BIN_:1)))
tot=$(jq -r '[.totals.runs, .totals.comp_blk_pct, .totals.fp_rate_pct, .totals.fn] | @tsv' <<<"$SUM")
read -r runs cb lk fp fn <<<"$(awk -F'\t' '{print $1, $2, $3, $4}' <<<"$tot")"
printf 'baseline: %d runs, 压缩 %d.%02d%%, 成功 job %d 个上假阳性行 %d (均值 %.1f 行/job), FN %d\n' \
  "$BN" $((bc/100)) $((bc%100)) "$BSC" "$BFP" "$(awk "BEGIN{print $BFP/$BSC}")" "$BFN"
printf 'skim:     %d runs, 压缩 %.2f%%, FP(failure-intent) 率 %.2f%%, FN %d\n' "$runs" "$cb" "$fp" "$fn"

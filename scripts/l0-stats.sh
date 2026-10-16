#!/usr/bin/env bash
# 职责: L0 统计 — intent 分层 FP/FN + 双粒度压缩率 + per-rule 分列 + rg 基线对照。
# 口径: spec §4 (failure-intent 才计 FP; rerun-through 排除; cancelled 只记命中不记缺失)。
set -euo pipefail
BIN="$(dirname "$0")/../engine/target/release/log-skimmer"
RL="$(dirname "$0")/../rules"
DL="$(dirname "$0")/../data/logs"

# --- 汇总态 ---
declare -A IN_T=() OUT_T=() SIG_T=() FP_T=() WN_T=() INF_T=() FN_T=() N_T=()
declare -A RULE_HIT=() RULE_FP=() RULE_FPX=() RULE_LINES=()   # per-rule: fragments / success-failure-intent-frags / 命中样本数 / 块行数
BASE_IN_T=0; BASE_OUT_T=0; BASE_FP=0; BASE_FN=0; BASE_N=0; BASE_SC=0

for j in "$DL"/*/*.log.json; do
  log="${j%.log.json}.log"; [ -f "$log" ] || continue
  con=$(jq -r .conclusion "$j"); att=$(jq -r '.attempt // 1' "$j"); repo=$(jq -r .repo "$j")
  # rerun-through: attempt>1 且最终成功 → 排除主指标
  if [ "$att" -gt 1 ] && [ "$con" = "success" ]; then continue; fi
  r=$("$BIN" skim "$log" --rules "$RL" --json 2>/dev/null) || r='{"blocks":[],"input_lines":0,"output_lines":0,"stats":{"signature_output_lines":0,"noise_lines_removed":0,"boundary_violations":[]}}'
  in=$(jq .input_lines <<<"$r"); out=$(jq .output_lines <<<"$r"); sig=$(jq .stats.signature_output_lines <<<"$r")
  # failure-intent frags (FP/FN 口径) 与 warning frags 分层
  fifrags=$(jq '[.blocks[].fragments[] | select(.intent=="failure")] | length' <<<"$r")
  wnfrags=$(jq '[.blocks[].fragments[] | select(.intent=="warning")] | length' <<<"$r")
  # per-rule
  while IFS=$'\t' read -r rid cnt lines; do
    RULE_HIT[$rid]=$(( ${RULE_HIT[$rid]:-0} + cnt ))
    RULE_LINES[$rid]=$(( ${RULE_LINES[$rid]:-0} + lines ))
  done < <(jq -r '.blocks[].fragments[] | [.rule, 1, (.block[1]-.block[0]+1)] | @tsv' <<<"$r")
  if [ "$con" = success ]; then
    FP_T[$repo]=$(( ${FP_T[$repo]:-0} + fifrags ))
    WN_T[$repo]=$(( ${WN_T[$repo]:-0} + wnfrags )); INF_T[$repo]=$(( ${INF_T[$repo]:-0} + $(jq "[.blocks[].fragments[] | select(.intent==\"info\")] | length" <<<"$r") ))
    BASE_SC=$((BASE_SC+1))
    # per-rule FP (failure-intent on success)
    while IFS=$'\t' read -r rid cnt; do
      RULE_FP[$rid]=$(( ${RULE_FP[$rid]:-0} + cnt ))
    done < <(jq -r '.blocks[].fragments[] | select(.intent=="failure") | [.rule, 1] | @tsv' <<<"$r")
  else
    [ "$fifrags" -eq 0 ] && FN_T[$repo]=$(( ${FN_T[$repo]:-0} + 1 ))
    # 命中样本数 (失败侧)
    while IFS=$'\t' read -r rid; do RULE_FPX[$rid]="${RULE_FPX[$rid]:-}>${repo}<"; done < <(jq -r '.blocks[].fragments[].rule' <<<"$r" | sort -u)
  fi
  key="$repo|$con"; N_T[$key]=$(( ${N_T[$key]:-0} + 1 ))
  IN_T[$key]=$(( ${IN_T[$key]:-0} + in )); OUT_T[$key]=$(( ${OUT_T[$key]:-0} + out )); SIG_T[$key]=$(( ${SIG_T[$key]:-0} + sig ))
  # --- rg 基线: error|warn 匹配行 ∪ tail -100 ---
  bl=$( { grep -ciE 'error|warn' "$log" 2>/dev/null || true; tail -100 "$log" | wc -l; } | awk 'NR==1{a=$1} NR==2{b=$1} END{print (a>b)?a:b}')
  # 基线压缩输出取两者较大 (粗基线: 匹配行数或尾部 100 行)
  BASE_IN_T=$((BASE_IN_T+in)); BASE_OUT_T=$((BASE_OUT_T+bl)); BASE_N=$((BASE_N+1))
  if [ "$con" = success ]; then
    b=$(grep -ciE 'error|warn' "$log" 2>/dev/null || true); BASE_FP=$((BASE_FP+b))
  else
    [ "$bl" -eq 0 ] && BASE_FN=$((BASE_FN+1))
  fi
done

echo "=== 汇总 (per repo × conclusion; rerun-through 已排除) ==="
printf '%-32s %-9s %5s %9s %9s %6s %6s %14s\n' repo concl runs 'comp%blk' 'comp%sig' FN 'FP(fail-int)' 'warn/info-hit(succ)'
for key in "${!N_T[@]}"; do
  repo="${key%|*}"; con="${key#*|}"; n=${N_T[$key]}; i=${IN_T[$key]}; o=${OUT_T[$key]}; s=${SIG_T[$key]}
  cb=$((10000*(i-o)/(i>0?i:1))); cs=$((10000*(i-s)/(i>0?i:1)))
  if [ "$con" = failure ]; then printf '%-32s %-9s %5d %6d.%02d%% %6d.%02d%% %6s %8s\n' "$repo" "$con" "$n" $((cb/100)) $((cb%100)) $((cs/100)) $((cs%100)) "${FN_T[$repo]:-0}" "-"
  else printf '%-32s %-9s %5d %9s %9s %6s %14s\n' "$repo" "$con" "$n" "-" "-" "-" "${FP_T[$repo]:-0} (${WN_T[$repo]:-0}w/${INF_T[$repo]:-0}i)"; fi
done | sort
echo
echo "=== per-rule (全语料; FP=failure-intent 在 success job 的 fragment 数) ==="
printf '%-40s %8s %8s %8s %5s\n' rule frags lines FP samples
for rid in "${!RULE_HIT[@]}"; do
  printf '%-40s %8d %8d %8d %5d\n' "$rid" "${RULE_HIT[$rid]}" "${RULE_LINES[$rid]}" "${RULE_FP[$rid]:-0}" "$(grep -o '>' <<<"${RULE_FPX[$rid]:-}" | wc -l)"
done | sort -k2 -rn
echo
echo "=== rg 基线 (error|warn ∪ tail-100) vs Log Skimmer ==="
bc=$((10000*(BASE_IN_T-BASE_OUT_T)/(BASE_IN_T>0?BASE_IN_T:1)))
printf 'baseline: %d runs, 压缩 %d.%02d%%, 成功 job %d 个上假阳性行 %d (均值 %.1f 行/job, 中招率见报告), FN %d\n' "$BASE_N" $((bc/100)) $((bc%100)) "$BASE_SC" "$BASE_FP" "$(awk "BEGIN{print $BASE_FP/$BASE_SC}")" "$BASE_FN"

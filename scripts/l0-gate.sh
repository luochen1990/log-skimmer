#!/usr/bin/env bash
# 职责: L0 门禁 (M1) — 全语料回归判定, 机器可读 JSON 输出到 stdout, 任一违例 exit 1:
#   0. 语料在场 ≥60 (防空转假绿: 语料路径漂移/误 ignore 时拒绝在空集上判定);
#   1. FN=0        (主指标; failure 家族样本上 failure-intent fragment 数为 0 的样本数, spec §4 不变量);
#   2. FP 率 ≤2%   (主指标全局; success 样本上 failure-intent fragment 的 block 行数 / success 输入总行数);
#   3. 边界违例=0  (全量; spec §4 实现哨兵, 非零 = 引擎 bug);
#   4. 零块=0      (全量; spec §3 结构承诺, 非零 = 规则集对某日志失明);
#   5. 基线防退化  (per repo × blk/sig 双粒度; 相对退化 >2% 判红; 改善 ≥2% 仅提示更新基线)。
# 阈值是验收线 (spec §0 召回优先), 禁止为过门禁放宽。
# 用法: scripts/l0-gate.sh [--update-baseline]
#   --update-baseline: 从当前实测重生成 data/l0-baseline.json 后退出 (规则/引擎/语料有意改进时)。
# 数据链: l0-collect.sh (采集) → l0-summarize.jq (口径) → 本脚本 (判定), 判定阈值 SSOT 在本文件。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BASELINE="${CORPUS_DIR:-$ROOT/data/logs}/l0-baseline.json" # 基线随语料仓版本化 (语料与基线恒配对; --update-baseline 亦写此处)
FP_RATE_MAX=2          # FP(failure-intent) 行数率上限 %
REGRESS_MAX_PCT=2      # 压缩率相对退化判红线 %
IMPROVE_HINT_PCT=2     # 压缩率相对改善提示线 % (不判红)

SUMMARY=$(bash "$ROOT/scripts/l0-collect.sh" | jq -s -f "$ROOT/scripts/l0-summarize.jq")

if [ "${1:-}" = "--update-baseline" ]; then
  # 基线生成在场断言: 管辖域 (ci=github) 样本 <50 拒绝写基线 — 防空语料生成
  #   "空基线" 被原子配对合法化 (2026-10-16 事故; 57 对现状留 7 对余量; 主仓
  #   判定模式仍是全量 ≥60, 两侧阈值语义不同故各自声明)
  n_github=$(jq -r '[.per_repo | to_entries[] | select(.value.ci == "github") | .value.runs] | add // 0' <<<"$SUMMARY")
  if [ "${n_github:-0}" -lt 50 ]; then
    echo "基线生成拒绝: 管辖域 (ci=github) 样本仅 ${n_github:-0} (<50), 疑语料缺失/拉取链路故障" >&2
    exit 1
  fi
  # 基线管辖域 = ci=github 的 repo (forgejo 侧是内网数据, 其 repo 键不出现在公网语料仓)
  jq -n --argjson s "$SUMMARY" '$s.per_repo | map_values(select(.ci == "github"))
    | map_values({runs, input_lines, comp_blk_pct, comp_sig_pct})
    | {comment: "l0-gate 压缩率基线 (per repo, 仅 ci=github 管辖域); 由 scripts/l0-gate.sh --update-baseline 生成, 勿手编", repos: .}' \
    > "$BASELINE"
  echo "基线已更新: $BASELINE" >&2
  exit 0
fi

BASE=$(cat "$BASELINE" 2>/dev/null || echo '{"repos":{}}')

RESULT=$(jq -n --argjson s "$SUMMARY" --argjson base "$BASE" \
  --argjson fp_max "$FP_RATE_MAX" --argjson reg_max "$REGRESS_MAX_PCT" --argjson imp "$IMPROVE_HINT_PCT" '
  def pctchg(new; old): if old > 0 then 100 * (new - old) / old else 0 end;
  def r2: (. * 100 | round) / 100;

  # --- 5. 基线对比 (per repo × blk/sig; 管辖域 = ci=github, forgejo 侧不进判定) ---
  ([$s.per_repo | to_entries[] | select(.value.ci == "github")
    | .key as $repo | .value as $cur | ($base.repos[$repo]) as $b |
    if $b == null then {repo: $repo, type: "new-repo", note: "语料新增 repo 无基线 (提示补基线, 不判红)"}
    else
      (pctchg($cur.comp_blk_pct; $b.comp_blk_pct)) as $dblk |
      (pctchg($cur.comp_sig_pct; $b.comp_sig_pct)) as $dsig |
      # 压缩率是越大越好: 相对变化为负且超过退化线 → 违例; 正向超过改善线 → 提示
      (if $dblk < -$reg_max then [{repo: $repo, type: "regression", grain: "blk",
         baseline_pct: ($b.comp_blk_pct | r2), current_pct: ($cur.comp_blk_pct | r2),
         rel_change_pct: ($dblk | r2)}] else [] end)
      + (if $dsig < -$reg_max then [{repo: $repo, type: "regression", grain: "sig",
         baseline_pct: ($b.comp_sig_pct | r2), current_pct: ($cur.comp_sig_pct | r2),
         rel_change_pct: ($dsig | r2)}] else [] end)
      + (if $dblk >= $imp then [{repo: $repo, type: "improved", grain: "blk", rel_change_pct: ($dblk | r2)}] else [] end)
      + (if $dsig >= $imp then [{repo: $repo, type: "improved", grain: "sig", rel_change_pct: ($dsig | r2)}] else [] end)
    end]
    + [$base.repos | to_entries[] | select(.key as $k | $s.per_repo | has($k) | not)
      | {repo: .key, type: "stale-baseline", note: "基线含语料已无的 repo (提示重生成基线; 语料仓管辖域内)"}]
    | flatten)   # to_entries[] 迭代 else 分支产数组, 收集后扁平化
  as $baseline_checks |

  # --- 0-4. 硬判定 ---
  # 0. 语料在场断言 (防空转通过: 语料路径漂移/误 ignore 时 collect 静默产出空集,
  #    全部指标恒 0 → verdict=pass 的假绿; 60 = 当前 67 对留余量, 语料增长无需改)
  (if $s.structural.samples_total < 60 then [{type: "corpus-missing",
     samples_total: $s.structural.samples_total,
     note: "语料样本数异常 (<60), 疑似 data/logs 路径漂移或被 ignore — 门禁拒绝在空语料上判定"}] else [] end)
  # 空基线硬防线: 管辖域有语料而基线为空 = 退化哨兵静默失效 (sidecar ci 缺失/基线管线
  #   同类回归), 拒绝无哨兵判定
  + (if (($base.repos | length) == 0) and ($s.per_repo | [to_entries[] | select(.value.ci == "github")] | length > 0)
   then [{type: "baseline-empty",
     note: "基线管辖域 (ci=github) 为空但语料在场 — 退化哨兵失效, 先 --update-baseline 再判定"}] else [] end)
  + (if $s.totals.fn > 0 then [{type: "fn",
     detail: ($s.per_repo_conclusion | to_entries | map(select(.value.fn > 0))
       | map({repo: .key, fn: .value.fn})),
     note: "失败样本零 failure-intent fragment (完全漏检, spec §0 召回优先不变量)"}] else [] end)
  + (if $s.totals.fp_rate_pct > $fp_max then [{type: "fp-rate",
     fp_rate_pct: ($s.totals.fp_rate_pct | r2), max_pct: $fp_max,
     fp_lines: $s.totals.fp_lines, success_input_lines: $s.totals.success_input_lines}] else [] end)
  + (if $s.structural.boundary_violations > 0 then [{type: "boundary-violation",
     count: $s.structural.boundary_violations, note: "实现哨兵: fragment 尾行未被任一 pattern 匹配"}] else [] end)
  + (if $s.structural.zero_block > 0 then [{type: "zero-block",
     count: $s.structural.zero_block, note: "结构承诺失守: 有样本 skim 零块 (规则集失明)"}] else [] end)
  + [$baseline_checks[] | select(.type == "regression")]
  as $violations |

  {verdict: (if ($violations | length) > 0 then "fail" else "pass" end),
   thresholds: {fp_rate_max_pct: $fp_max, regression_max_pct: $reg_max, improve_hint_pct: $imp},
   violations: $violations,
   notices: [$baseline_checks[] | select(.type == "new-repo" or .type == "stale-baseline" or .type == "improved")],
   summary: $s}
') || { printf '%s\n' "$RESULT" >&2; exit 1; }
echo "$RESULT"  # stdout: 机器可读判定
echo "l0-gate: $(jq -r '"verdict=\(.verdict) FN=\(.summary.totals.fn) FP率=\(.summary.totals.fp_rate_pct|.*100|round/100)% 零块=\(.summary.structural.zero_block) 边界违例=\(.summary.structural.boundary_violations) 压缩 blk=\(.summary.totals.comp_blk_pct|.*100|round/100)% sig=\(.summary.totals.comp_sig_pct|.*100|round/100)%"' <<<"$RESULT")" >&2
jq -e '.verdict == "pass"' <<<"$RESULT" > /dev/null

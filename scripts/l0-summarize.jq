# 职责: L0 统计口径 SSOT — 消费 l0-collect.sh 的 JSONL (jq -s -f 调用), 产出三层聚合:
#   per_repo_conclusion (repo×conclusion) / per_repo / totals (主指标) + per_rule + structural。
# 口径 (spec §4, 沿用 POC l0-stats.sh 现状):
#   - 主指标滤除 rerun-through (excluded=true);
#   - FN = failure 家族 (failure/timed_out) 样本上 failure-intent fragment 数为 0 的样本数
#     (cancelled 截断日志不计缺失, spec §4 三态; skipped 排除见 collect);
#   - FP = success 样本上 failure-intent fragment 的 block 行数与计数, 率分母 = 该聚合 success 样本输入行;
#   - 零块/边界违例是全量统计 (spec §3 结构承诺 / §4 实现哨兵语义, 不滤 excluded);
#   - 压缩率 blk = 1-Σout/Σin, sig = 1-Σsig/Σin (聚合行数比, 非样本内率的平均)。
# 消费方: scripts/l0-gate.sh (门禁判定) 与 scripts/l0-stats.sh (人类报表) — 二者只做渲染,
#   任何口径演进出改本文件 (勿在消费端复写判定条件, 防门禁与报表分叉)。

def ratio(n; d): if (d // 0) > 0 then (100 * n / d) else 0 end;

def agg: {
  runs: length,
  input_lines: (map(.input_lines) | add // 0),
  output_lines: (map(.output_lines) | add // 0),
  signature_output_lines: (map(.signature_output_lines) | add // 0),
  # FN 谓词只数 failure/timed_out (spec §4: cancelled 截断日志未命中不计 FN;
  # skipped 排除口径见 collect; 非 success 且非 failure 家族的 cancelled 单列为弱信号)
  fn: (map(select((.conclusion == "failure" or .conclusion == "timed_out") and (.failure_frags == 0))) | length),
  success_input_lines: (map(select(.conclusion == "success").input_lines) | add // 0),
  fp_frags: (map(select(.conclusion == "success").failure_frags) | add // 0),
  fp_lines: (map(select(.conclusion == "success").failure_frag_lines) | add // 0),
  success_warning_frags: (map(select(.conclusion == "success").warning_frags) | add // 0),
  success_info_frags: (map(select(.conclusion == "success").info_frags) | add // 0)
};

def with_rates: . + {
  comp_blk_pct: ratio(.input_lines - .output_lines; .input_lines),
  comp_sig_pct: ratio(.input_lines - .signature_output_lines; .input_lines),
  fp_rate_pct: ratio(.fp_lines; .success_input_lines)
};

map(select(.excluded | not)) as $m
| {
    per_repo_conclusion: ($m | group_by(.repo + "|" + .conclusion)
      | map({key: (.[0].repo + "|" + .[0].conclusion), value: (agg | with_rates)}) | from_entries),
    per_repo: ($m | group_by(.repo)
      # ci 域标记随行 (基线管辖域分界: 仅 ci=github 进公网基线/退化判定 —
      # forgejo 侧数据不出内网, 其压缩率只进报表不进基线; FN/FP/零块/边界仍全量)
      | map({key: .[0].repo, value: ((agg | with_rates) + {ci: .[0].ci})}) | from_entries),
    totals: ($m | agg | with_rates),
    per_rule: ([$m | .[] | . as $s | .frags[]
        | {rule, intent, lines, repo: $s.repo, con: $s.conclusion}]
      | group_by(.rule)
      | map({key: .[0].rule, value: {
          frags: length,
          lines: (map(.lines) | add // 0),
          fp_frags: (map(select(.con == "success" and .intent == "failure")) | length),
          hit_repos: (map(select(.con != "success").repo) | unique | length)
        }}) | from_entries),
    structural: {
      zero_block: (map(.zero_block) | add // 0),
      boundary_violations: (map(.boundary_violations) | add // 0),
      samples_total: length,
      samples_excluded: (map(select(.excluded)) | length)
    }
  }

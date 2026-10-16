#!/usr/bin/env bash
# verify-rules.sh — 规则集自验 (Log Skimmer POC)
#
# 职责边界: 对 rules/**.yaml 做三层机械校验, 不做 IO 之外的任何事:
#   1) 结构 lint: 必填字段/枚举值/id 前缀约束 (spec §1 加载期校验的本地近似);
#   2) 正则编译: 全部 patterns[].regex 经 grep -P (PCRE) 编译探测 (RE2 语法子集的代理,
#      本规则集未用 lookahead/backref, PCRE 编译通过 + 人工复查即可);
#   3) 锚行命中统计: patterns[0] 对 data/logs 全样本逐文件 grep 计数 —
#      失败日志命中 = 召回证据, 成功日志命中 = FP 代理 (应为 0)。
# 依赖: bash + grep -P; 无 LLM、无网络。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RULES_DIR="$ROOT/rules"
LOGS_DIR="$ROOT/data/logs"
fail=0

lint_rule_file() { # lint_rule_file <yaml>; 违例打印并置 fail
  local f=$1 id= tool= kind= line
  while IFS= read -r line; do
    case $line in
      '#'* | '') ;;
      id:*) id=${line#id: } ;;
      tool:*) tool=${line#tool: } ;;
      kind:*) kind=${line#kind: } ;;
      'patterns:' | '- regex:'* | '  - regex:'* | '    regex:'* | '    loop:'*) ;;
      scope:* | repo:* | category:* | confidence:* | title:* | 'context:'* | 'structured-hint:'* | 'attribution:'*) ;;
      *)
        echo "LINT-FAIL $f: 无法识别的行: $line"
        fail=1
        ;;
      esac
  done <"$f"
  if [[ -z ${id:-} || -z ${tool:-} || -z ${kind:-} ]]; then
    echo "LINT-FAIL $f: 缺必填 id/tool/kind"
    fail=1
  fi
  if [[ ${id:-} != ^[a-z0-9-]+$ ]] && ! [[ ${id:-} =~ ^[a-z0-9-]+$ ]]; then
    echo "LINT-FAIL $f: id 不匹配 ^[a-z0-9-]+$: $id"
    fail=1
  fi
  if [[ -n ${id:-} && -n ${tool:-} && $id != "$tool"-* ]]; then
    echo "LINT-FAIL $f: id 未以 tool 值为前缀: id=$id tool=$tool"
    fail=1
  fi
  if [[ ${kind:-} != signal && ${kind:-} != noise ]]; then
    echo "LINT-FAIL $f: kind 非法: $kind"
    fail=1
  fi
}

extract_regexes() { # extract_regexes <yaml> -> 逐行输出 YAML 单引号串 (已反转义 '')
  sed -n "s/^[[:space:]]*-[[:space:]]*regex: *'\(.*\)'[[:space:]]*$/\1/p" "$1" | sed "s/''/'/g"
}

echo '=== 1) 结构 lint + 2) 正则编译 (PCRE 代理) ==='
shopt -s nullglob
rule_files=("$RULES_DIR"/*/*.yaml)
for f in "${rule_files[@]}"; do
  lint_rule_file "$f"
  i=0
  while IFS= read -r re; do
    i=$((i + 1))
    # grep 退出码: 0=命中 1=无命中 2=错误; 仅 2 是编译失败 (输入 'x' 不匹配大多数锚属正常)
    echo x | grep -aP -- "$re" >/dev/null 2>&1 || rc=$?
    rc=${rc:-0}
    if ((rc == 2)); then
      echo "REGEX-FAIL $f patterns[$i] 编译失败: $re"
      fail=1
    fi
    rc=0
  done < <(extract_regexes "$f")
  if ((i == 0)); then
    echo "LINT-FAIL $f: patterns 为空"
    fail=1
  fi
done
echo "lint/编译检查: ${#rule_files[@]} 个规则文件, fail=$fail"

echo
echo '=== 3) 锚行命中统计 (patterns[0] 对全样本) ==='
printf '%-36s %-6s %s\n' 'rule' '锚命中' '来源文件 (失败日志: 计数)'
for f in "${rule_files[@]}"; do
  id=$(sed -n 's/^id: *//p' "$f")
  anchor=$(extract_regexes "$f" | head -1)
  total=0
  hits=""
  for log in "$LOGS_DIR"/*/*.log; do
    # 剥离 CRLF 的 \r 再匹配 — 与引擎逐行消费语义对齐 (Rust lines() 剥 \r)
    c=$(sed 's/\r$//' "$log" | grep -acP -- "$anchor" || true)
    if ((c > 0)); then
      base=${log##*/}
      slug=${log%"$base"}
      slug=${slug%/}
      slug=${slug##*/}
      concl=$(jq -r .conclusion "${log}.json" 2>/dev/null || echo '?')
      hits+="$slug/${base%.log}:$c($concl) "
      if [[ $concl == failure ]]; then total=$((total + c)); fi
    fi
  done
  printf '%-36s %-6s %s\n' "$id" "$total" "$hits"
done

exit $fail

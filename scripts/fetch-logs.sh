#!/usr/bin/env bash
# fetch-logs.sh — 幂等抓取三仓 GitHub Actions 日志语料 (Log Skimmer POC)
#
# 职责边界: 拉取失败/成功 run 的 job 日志到 data/logs/<owner>__<repo>/,
#   每 run 只取一个 job (失败 run 取第一个 conclusion=failure 的 job, 成功 run 取第一个
#   conclusion=success 的 job, 无则 jobs[0]), 产 .log + .json sidecar。
#   已存在的 run (目录下有 <run_id>-*.log) 直接计入配额跳过 — 幂等。
# 配额: 每仓失败 run >=5 (status=failure 翻页不足 40 候选时, 放宽为全量列表客户端过滤,
#   最多翻 10 页); 成功 run =5。
# 韧性: 日志端点 404/410 (过期清理) / 无匹配 job / API 失败 → 跳过该 run 换下一个, 不计配额。
# 依赖: bash + curl(-L) + jq; env GITHUB_API_TOKEN (必需)。
set -euo pipefail

API="https://api.github.com"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_DIR="$ROOT/data/logs"
WANT_FAIL=${WANT_FAIL:-5}
WANT_SUCCESS=${WANT_SUCCESS:-5}
MAX_CANDIDATES=40 # 每仓候选 run 上限 (留冗余吸收 404 跳过)
REPOS=(sst/opencode niri-wm/niri nix-community/home-manager)

log() { printf '[fetch %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

api_get() { # api_get <path> -> JSON body (stdout); HTTP 错误时输出为空
  curl -sfSL --max-time 90 --retry 2 \
    -H "Authorization: Bearer $GITHUB_API_TOKEN" \
    -H "Accept: application/vnd.github+json" \
    "$API/$1" || true
}

download_log() { # download_log <jobs-api-path> <dest>; 0=成功落盘
  local code
  code=$(curl -sSL --max-time 180 --retry 1 \
    -H "Authorization: Bearer $GITHUB_API_TOKEN" \
    -o "$2.tmp" -w '%{http_code}' "$API/$1" || true)
  if [[ "$code" == 200 && -s "$2.tmp" ]]; then
    mv "$2.tmp" "$2"
    return 0
  fi
  rm -f "$2.tmp"
  log "    日志端点不可用 (http=${code:-ERR}), 跳过该 job"
  return 1
}

existing_count() { # existing_count <dir> <conclusion> -> 已有样本 run 数 (按 run_id 去重)
  local dir=$1 c=$2 n
  [[ -d $dir ]] || { echo 0; return; }
  n=$(jq -rs --arg c "$c" 'map(select(.conclusion == $c)) | map(.run_id) | unique | length' \
    "$dir"/*.json 2>/dev/null || true)
  echo "${n:-0}"
}

run_already_fetched() { # run_already_fetched <dir> <run_id>; 0=已存在
  # 注: 本机 bash 无 compgen (编译禁用 completion), 用 glob 判断
  local f
  for f in "$1"/"$2"-*.log; do
    if [[ -f $f ]]; then return 0; fi
  done
  return 1
}

runs_to_tsv() { # runs_to_tsv <status-filter-or-empty> — 列表响应转 "id<TAB>name" 行, 客户端过滤 failure
  local status=$1
  jq -r --arg s "$status" '
    .workflow_runs // []
    | if $s == "failure" then map(select(.conclusion == "failure"))
      elif $s == "success" then map(select(.conclusion == "success"))
      else . end
    | sort_by(-.id) | .[] | "\(.id)\t\(.name // .path // "?")"'
}

list_failure_candidates() { # list_failure_candidates <repo> — 失败 run 候选 (新优先, 去重保序)
  local repo=$1 page batch lines=''
  # 通道 1: status=failure 直查 (翻 ≤3 页, 够 MAX_CANDIDATES 即停)
  for page in 1 2 3; do
    batch=$(api_get "repos/$repo/actions/runs?status=failure&per_page=100&page=$page" | runs_to_tsv failure)
    [[ -n $batch ]] && lines+=$'\n'$batch
    if [[ $(grep -c . <<<"$lines" || true) -ge $MAX_CANDIDATES ]]; then break; fi
  done
  # 通道 2 (候选不足时放宽): 全量列表客户端过滤, 深翻 ≤10 页
  if [[ $(grep -c . <<<"$lines" || true) -lt $MAX_CANDIDATES ]]; then
    log "  status=failure 候选不足, 放宽为全量列表客户端过滤"
    for page in 1 2 3 4 5 6 7 8 9 10; do
      batch=$(api_get "repos/$repo/actions/runs?per_page=100&page=$page" | runs_to_tsv failure)
      [[ -n $batch ]] && lines+=$'\n'$batch
      if [[ $(grep -c . <<<"$lines" || true) -ge $MAX_CANDIDATES ]]; then break; fi
    done
  fi
  awk -F'\t' 'NF==2 && !seen[$1]++' <<<"$lines"
}

process_run() { # process_run <repo> <dir> <run_id> <wf_name> <failure|success>
  # 返回: 0=新落盘 (配额 +1); 1=跳过; 2=早已存在 (已计入初始存量, 勿重复计数)
  local repo=$1 dir=$2 run_id=$3 wf=$4 concl=$5 jobs job_id out
  if run_already_fetched "$dir" "$run_id"; then
    log "  run $run_id 样本已存在 (已计入初始存量)"
    return 2
  fi
  jobs=$(api_get "repos/$repo/actions/runs/$run_id/jobs")
  if [[ -z $jobs ]]; then
    log "  run $run_id jobs 查询失败, 跳过"
    return 1
  fi
  if [[ $concl == failure ]]; then
    job_id=$(jq -r '([.jobs[]? | select(.conclusion == "failure")][0].id // empty)' <<<"$jobs")
  else
    job_id=$(jq -r '([.jobs[]? | select(.conclusion == "success")][0].id // .jobs[0].id // empty)' <<<"$jobs")
  fi
  if [[ -z $job_id ]]; then
    log "  run $run_id 无 ${concl} job, 跳过"
    return 1
  fi
  out="$dir/$run_id-$job_id.log"
  if download_log "repos/$repo/actions/jobs/$job_id/logs" "$out"; then
    jq -n --arg repo "$repo" --argjson run_id "$run_id" --argjson job_id "$job_id" \
      --arg concl "$concl" --arg wf "$wf" --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      '{repo:$repo, run_id:$run_id, job_id:$job_id, conclusion:$concl, workflow_name:$wf, fetched_at:$at}' \
      >"$out.json"
    log "  已存 run=$run_id job=$job_id (${concl}, $(wc -l <"$out") 行, wf=${wf})"
    return 0
  fi
  return 1
}

fetch_batch() { # fetch_batch <repo> <dir> <concl:failure|success> <want>
  local repo=$1 dir=$2 concl=$3 want=$4 have line run_id wf
  have=$(existing_count "$dir" "$concl")
  log "$concl run: 已有 $have / 目标 $want"
  if ((have < want)); then
    local -a cands=()
    if [[ $concl == failure ]]; then
      mapfile -t cands < <(list_failure_candidates "$repo" | head -n "$MAX_CANDIDATES")
    else
      mapfile -t cands < <(api_get "repos/$repo/actions/runs?status=success&per_page=100" |
        runs_to_tsv success | head -n "$MAX_CANDIDATES")
    fi
    log "候选 run: ${#cands[@]} 个"
    for line in "${cands[@]}"; do
      if ((have >= want)); then break; fi
      [[ -z $line ]] && continue
      run_id=${line%%$'\t'*}
      wf=${line#*$'\t'}
      if process_run "$repo" "$dir" "$run_id" "$wf" "$concl"; then
        have=$((have + 1)) # 仅新落盘才计; 返回 2 (已存在) 已含于初始存量, 不重复计
      fi
    done
  fi
  log "$concl run 最终: $have (目标 $want)"
}

main() {
  : "${GITHUB_API_TOKEN:?需要 env GITHUB_API_TOKEN}"
  mkdir -p "$DATA_DIR"
  local repo slug dir
  for repo in "${REPOS[@]}"; do
    slug=${repo//\//__}
    dir="$DATA_DIR/$slug"
    mkdir -p "$dir"
    log "== $repo =="
    fetch_batch "$repo" "$dir" failure "$WANT_FAIL"
    fetch_batch "$repo" "$dir" success "$WANT_SUCCESS"
  done
  log "== 汇总 =="
  for repo in "${REPOS[@]}"; do
    slug=${repo//\//__}
    dir="$DATA_DIR/$slug"
    log "$slug: failure=$(existing_count "$dir" failure) success=$(existing_count "$dir" success) (文件对 $(find "$dir" -maxdepth 1 -name '*.json' | wc -l))"
  done
}

main "$@"

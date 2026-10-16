#!/usr/bin/env bash
# fetch-logs.sh — 幂等抓取三仓 GitHub Actions 日志语料 (Log Skimmer POC)
#
# 职责边界: 拉取失败/成功 run 的 job 日志到 data/logs/<owner>__<repo>/,
#   每 run 只取一个 job (失败 run 取第一个 conclusion=failure 的 job, 成功 run 取第一个
#   conclusion=success 的 job, 无则 jobs[0]), 产 .log + .json sidecar。
#   已存在的 run (目录下 <run_id>-*.log 在场) 直接计入配额跳过 — 幂等。
# 配额: 槽位约定 **14 失败 + 5 成功 per repo** (SSOT 在此默认值 — corpus-sync 与
#   语料仓 GHA refresh 均不传参走默认); 翻页不足 40 候选时放宽为全量列表客户端
#   过滤, 最多翻 10 页。
# 韧性: 日志端点 404/410 (过期清理) / 无匹配 job / API 失败 → 跳过该 run 换下一个, 不计配额。
# 依赖: bash + curl(-L) + jq; env GITHUB_API_TOKEN (必需)。
set -euo pipefail

API="https://api.github.com"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_DIR="${CORPUS_DIR:-$ROOT/data/logs}" # 语料根 (参数化: 语料仓 GHA 刷新直接写仓内)
WANT_FAIL=${WANT_FAIL:-14}
WANT_SUCCESS=${WANT_SUCCESS:-5}
MAX_CANDIDATES=40 # 每仓候选 run 上限 (留冗余吸收 404 跳过)
REPOS=(anomalyco/opencode niri-wm/niri nix-community/home-manager)

log() { printf '[fetch %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

api_get() { # api_get <path> -> JSON body (stdout); HTTP 错误时输出为空
  curl -sfSL --max-time 90 --retry 2 \
    -H "Authorization: Bearer $GITHUB_API_TOKEN" \
    -H "Accept: application/vnd.github+json" \
    "$API/$1" || true
}

download_log() { # download_log <jobs-api-path> <dest>; 0=成功落盘
  local code
  # 输出父目录前置保证 (缺目录时 curl -o 失败会被下方 || true 吞成空文件形态)
  mkdir -p "$(dirname "$2")"
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
    $(find "$dir" -name '*.log.json' 2>/dev/null) 2>/dev/null || true)
  echo "${n:-0}"
}

run_already_fetched() { # run_already_fetched <dir> <run_id>; 0=已存在
  # 递归匹配嵌套布局下任意日期目录的 <run_id>-*.log
  find "$1" -name "$2-*.log" -print -quit 2>/dev/null | grep -q .
}

runs_to_tsv() { # runs_to_tsv <status-filter-or-empty> — 列表响应转 "id<TAB>name" 行, 客户端过滤 failure
  local status=$1
  jq -r --arg s "$status" '
    .workflow_runs // []
    | if $s == "failure" then map(select(.conclusion == "failure"))
      elif $s == "success" then map(select(.conclusion == "success"))
      else . end
    # 年龄过滤: 跳过 <10min 的 run — 正在跑/刚完成的 run 日志 blob 未就绪 (BlobNotFound);
    #   completed 的 blob 分钟级就绪, 10min 保守余量
    | map(select((.created_at // "1970-01-01T00:00:00Z" | fromdateiso8601) < (now - 600)))
    | sort_by(-.id) | .[] | "\(.id)\t\(.created_at // "?")\t\(.name // .path // "?")"'
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
  # NF==3: TSV 三列 (id\tcreated\tname) — 布局迁移加列时曾漏改 NF==2 致候选恒空
  awk -F'\t' 'NF==3 && !seen[$1]++' <<<"$lines"
}

process_run() { # process_run <repo> <dir> <run_id> <wf_name> <failure|success>
  # 返回: 0=新落盘 (配额 +1); 1=跳过; 2=早已存在 (已计入初始存量, 勿重复计数)
  local repo=$1 dir=$2 run_id=$3 wf=$4 concl=$5 created=$6 jobs job_id jname date out
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
  jname=$(jq -r --argjson id "$job_id" '.jobs[]? | select(.id==$id) | .name // "?"' <<<"$jobs" | head -1)
  jname=$(tr '/ ' '--' <<<"${jname:-unknown}" | cut -c1-30)
  date=${created%%T*}; [ -n "$date" ] || date=unknown
  out="$dir/$date/$run_id-$job_id-$jname-$concl.log"
  # 嵌套布局 date 目录必须先建 (2026-10-16 事故: 布局迁移加 $date/ 层而未建目录,
  #   curl -o 打开输出文件失败被 || true 吞成 "http=200 空文件", 全候选静默跳过)
  mkdir -p "$dir/$date"
  if download_log "repos/$repo/actions/jobs/$job_id/logs" "$out"; then
    jq -n --arg repo "github.com/$repo" --argjson run_id "$run_id" --argjson job_id "$job_id" \
      --arg concl "$concl" --arg wf "$wf" --arg ca "$created" --arg jn "$jname" --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      '{repo:$repo, run_id:$run_id, job_id:$job_id, conclusion:$concl, workflow_name:$wf, created_at:$ca, job_name:$jn, ci:"github", fetched_at:$at}' \
      >"$out.json"
    log "  已存 run=$run_id job=$job_id (${concl}, $(wc -l <"$out") 行, wf=${wf})"
    return 0
  fi
  return 1
}

fetch_batch() { # fetch_batch <repo> <dir> <concl:failure|success> <want>
  local repo=$1 dir=$2 concl=$3 want=$4 have line run_id created rest wf
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
      run_id=${line%%$'\t'*}; rest=${line#*$'\t'}
      created=${rest%%$'\t'*}; wf=${rest#*$'\t'}
      if process_run "$repo" "$dir" "$run_id" "$wf" "$concl" "$created"; then
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
    dir="$DATA_DIR/github.com/$repo" # 嵌套布局 host/owner/repo/<date>/ (可读性优先, 弃 __ 扁平化)
    mkdir -p "$dir"
    log "== $repo =="
    fetch_batch "$repo" "$dir" failure "$WANT_FAIL"
    fetch_batch "$repo" "$dir" success "$WANT_SUCCESS"
  done
  log "== 汇总 =="
  local empty=0
  for repo in "${REPOS[@]}"; do
    dir="$DATA_DIR/github.com/$repo"
    n=$(find "$dir" -name '*.log.json' | wc -l)
    log "$repo: failure=$(existing_count "$dir" failure) success=$(existing_count "$dir" success) (文件对 $n)"
    # 空仓硬防线: 任一仓 0 对在场 = 拉取链路整体故障 (认证/网络/API), 拒绝以
    #   "韧性逐条跳过" 之名把空语料合法化 (2026-10-16 事故: GHA per-run token
    #   跨仓 401 → 全跳过 → exit 0 → 空仓被 commit)。已满配额的幂等重跑不受影响
    #   (existing > 0)。
    if [ "$n" -eq 0 ]; then
      log "ERROR: $repo 语料为 0 对 — 拉取链路故障, 拒绝空仓收场"
      empty=1
    fi
  done
  [ "$empty" -eq 0 ] || exit 1
}

main "$@"

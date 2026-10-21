#!/usr/bin/env bash
# fetch-logs.sh — 幂等抓取 GitHub Actions job 日志语料 (槽位滚动抓取器)
#
# 职责边界: 按项目清单拉取最近失败/成功 run 的 job 日志到
#   data/logs/github.com/<owner>/<repo>/<date>/。每 run 取一个 job: 失败 run 取
#   第一个 conclusion=failure 的 job, 成功 run 取第一个 success job (无则 jobs[0]);
#   候选 job 日志端点 404/410 或超限 → 顺延同 run 下一个同结论 job。
# 产出与配对约定: 样本 <run_id>-<job_id>-<job_name>-<conclusion>.log.gz (gzip,
#   证据原样不截断); sidecar = 样本名去 .gz 后缀 + .json (<...>.log.json), 字段
#   {repo, run_id, job_id, conclusion, workflow_name, created_at, job_name,
#   attempt, ci, fetched_at, gate_exempt} — gate_exempt: 项目清单 exempt 字段
#   在场 → true, 否则 false (无 --projects 时恒 false)。
# 项目清单: --projects <yaml> (schema 见 usage; slots/max 项目级覆盖 defaults);
#   无参数走内置三仓默认 (向后兼容, corpus 仓 GHA 刷新路径; 槽位 14F+5S SSOT
#   在此默认值, WANT_FAIL/WANT_SUCCESS env 可覆盖)。yaml 解析选型 yq: devShell
#   与 GHA ubuntu runner image 均预装 (Ubuntu2404-Readme Tools: yq, 同 4.x);
#   python3 无 PyYAML、jq 无 YAML 支持, 均非开箱可用。
# 槽位来源过滤: BOT_WF_PATTERNS (下方可编辑清单, 子串匹配且大小写不敏感的
#   workflow name) 命中的 run — pages 部署/dependabot/stale 等 bot 噪声 — 不作槽位来源。
# 大 job 上限: 单 job 明文日志 > max_job_log_bytes → 跳过该 job 并 stderr 注记,
#   不截断 (证据完整性优先; 下载后计量 — API 不暴露日志体积)。
# 韧性: 单项目失败 (404/无 run/rate limit) 注记跳过不中断整体; 全部项目 0 对
#   才 exit 1 (空语料硬防线 — 2026-10-16 事故: 跨仓 401 全跳过仍 exit 0, 空仓
#   被 commit。"任一仓空即红" 旧口径随多项目规模化放宽为全空口径: 全空才是
#   链路整体故障信号, 部分空为逐项目注记的 WARN)。
# 依赖: bash + curl(-L) + jq (+ yq, 仅 --projects); env GITHUB_API_TOKEN (必需)。
set -euo pipefail

API="https://api.github.com"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_DIR="${CORPUS_DIR:-$ROOT/data/logs}"                 # 语料根 (参数化: 语料仓 GHA 刷新直接写仓内)
WANT_FAIL=${WANT_FAIL:-14}                                # 无 --projects 时默认槽位 (env 可覆盖)
WANT_SUCCESS=${WANT_SUCCESS:-5}
MAX_JOB_LOG_BYTES=${MAX_JOB_LOG_BYTES:-5242880}           # 单 job 明文日志上限 (超限跳过不截断)
MAX_CANDIDATES=40                                         # 每仓候选 run 上限 (留冗余吸收 404/超限跳过)
REPOS=(anomalyco/opencode niri-wm/niri nix-community/home-manager) # 无 --projects 默认清单
# bot/噪声 workflow 名清单 (子串匹配, 大小写不敏感) — 命中者不作槽位来源; 增删只改这里
BOT_WF_PATTERNS=('pages build and deployment' 'dependabot' 'stale' 'renovate')
# → jq test() 用的 alternation 正则; 清单被清空时用永不匹配正则兜底 (空正则匹配一切 = 全过滤灾难)
BOT_WF_RE=$(IFS='|'; printf '%s' "${BOT_WF_PATTERNS[*]}")
[[ -n $BOT_WF_RE ]] || BOT_WF_RE='[^\s\S]'

DRY_RUN=0 LIMIT='' PROJECTS_FILE=''

log() { printf '[fetch %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

usage() {
  cat <<'EOF'
用法: fetch-logs.sh [--projects <yaml>] [--dry-run] [--limit N]

  (无参数)     内置三仓默认清单, 槽位 WANT_FAIL/WANT_SUCCESS env 可覆盖
  --projects   项目清单 yaml, schema:
                 defaults: {slots: {failure: N, success: M}, max_job_log_bytes: B}
                 projects: [{repo: owner/name, slots?: {...}, max_job_log_bytes?: B,
                            exempt?: <任意字符串理由>}]
               (slots/max 项目级覆盖 defaults; exempt 在场 → sidecar gate_exempt=true)
  --dry-run    只列各项目将选中的 run/job (元数据查询, 不下载日志)
  --limit N    只处理清单前 N 个项目 (0 = 不处理任何项目, 冒烟用)
env: GITHUB_API_TOKEN (必需) / CORPUS_DIR; 槽位与上限 env (WANT_FAIL / WANT_SUCCESS /
     MAX_JOB_LOG_BYTES) 仅无 --projects 时生效 (--projects 模式槽位全由清单决定)
EOF
}

parse_args() {
  while (($#)); do
    case $1 in
      --projects)
        [[ $# -ge 2 ]] || { echo "--projects 需要参数" >&2; exit 2; }
        [[ -f $2 ]] || { echo "清单文件不存在: $2" >&2; exit 2; }
        PROJECTS_FILE=$2; shift 2;;
      --dry-run) DRY_RUN=1; shift;;
      --limit)
        [[ $# -ge 2 && $2 =~ ^[0-9]+$ ]] || { echo "--limit 需要非负整数参数" >&2; exit 2; }
        LIMIT=$2; shift 2;;
      -h|--help) usage; exit 0;;
      *) echo "未知参数: $1" >&2; usage >&2; exit 2;;
    esac
  done
}

plan_projects() { # → stdout: "repo\tfailure槽\tsuccess槽\tmax_bytes\texempt(true|false)" 行
  # 清单形态 SSOT: --projects (yaml) 与内置三仓 (env 槽位) 双源在此归一为同构 TSV
  if [[ -n $PROJECTS_FILE ]]; then
    # shellcheck disable=SC2016  # $df/$ds/$dm 是 yq 变量, 非待展开的 shell 变量
    yq -r '
      .defaults.slots.failure as $df | .defaults.slots.success as $ds
      | .defaults.max_job_log_bytes as $dm
      | .projects[]
      | [.repo, (.slots.failure // $df), (.slots.success // $ds),
         (.max_job_log_bytes // $dm), (.exempt != null)] | @tsv' "$PROJECTS_FILE"
  else
    local repo
    for repo in "${REPOS[@]}"; do
      printf '%s\t%s\t%s\t%s\tfalse\n' "$repo" "$WANT_FAIL" "$WANT_SUCCESS" "$MAX_JOB_LOG_BYTES"
    done
  fi
}

validate_plan() { # validate_plan <tsv行...> — 入口集中校验 (核心逻辑此后可假数据合规)
  local line repo f s m e bad=0
  for line in "$@"; do
    IFS=$'\t' read -r repo f s m e <<<"$line"
    # repo 形态 owner/name 白名单: 结构性阻断路径逃逸 (../) 与 URL 注入, 兼作 yq null 值拦截
    if [[ ! $repo =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ || ! $f =~ ^[0-9]+$ || ! $s =~ ^[0-9]+$ \
          || ! $m =~ ^[0-9]+$ || ! $e =~ ^(true|false)$ ]]; then
      log "ERROR: 清单行不合规 (需 owner/name\t数字\t数字\t数字\ttrue|false): $line"
      bad=1
    fi
  done
  ((bad == 0)) || exit 2
}

api_get() { # api_get <path> -> JSON body (stdout); HTTP 错误时输出为空
  curl -sfSL --max-time 90 --retry 2 \
    -H "Authorization: Bearer $GITHUB_API_TOKEN" \
    -H "Accept: application/vnd.github+json" \
    "$API/$1" || true
}

download_log() { # download_log <jobs-api-path> <dest.log.gz> <max_bytes>
  # 返回: 0=成功; 1=端点不可用; 3=超限。日志端点重定向 blob 为明文, 本地 gzip。
  # 输出父目录前置保证 (缺目录时 curl -o 失败会被下方 || true 吞成空文件形态)
  local code size
  mkdir -p "$(dirname "$2")"
  code=$(curl -sSL --max-time 180 --retry 1 \
    -H "Authorization: Bearer $GITHUB_API_TOKEN" \
    -o "$2.tmp" -w '%{http_code}' "$API/$1" || true)
  if [[ "$code" == 200 && -s "$2.tmp" ]]; then
    size=$(wc -c <"$2.tmp")
    if ((size > $3)); then
      rm -f "$2.tmp"
      log "    job 日志 ${size}B 超上限 ${3}B, 跳过该 job (不截断, 证据完整性优先)"
      return 3
    fi
    if gzip -c "$2.tmp" >"$2"; then rm -f "$2.tmp"; return 0; fi
  fi
  rm -f "$2.tmp"
  log "    日志端点不可用 (http=${code:-ERR}), 跳过该 job"
  return 1
}

existing_count() { # existing_count <dir> <conclusion> -> 已有样本 run 数 (按 run_id 去重)
  local dir=$1 c=$2 n
  [[ -d $dir ]] || { echo 0; return; }
  # shellcheck disable=SC2046  # 有意分词: sidecar 路径列表作 jq 多文件参数 (路径已清洗无空白)
  n=$(jq -rs --arg c "$c" 'map(select(.conclusion == $c)) | map(.run_id) | unique | length' \
    $(find "$dir" -name '*.log.json' 2>/dev/null) 2>/dev/null || true)
  echo "${n:-0}"
}

run_already_fetched() { # run_already_fetched <dir> <run_id>; 0=已存在
  # 递归匹配嵌套布局下任意日期目录的 <run_id>-*.log / *.log.gz (兼容明文旧语料与 gz 新语料)
  find "$1" \( -name "$2-*.log" -o -name "$2-*.log.gz" \) -print -quit 2>/dev/null | grep -q .
}

runs_to_tsv() { # runs_to_tsv <status-filter-or-empty> — 列表响应转 "id<TAB>created<TAB>name" 行, 客户端过滤 failure
  local status=$1
  jq -r --arg s "$status" --arg bot "$BOT_WF_RE" '
    .workflow_runs // []
    | if $s == "failure" then map(select(.conclusion == "failure"))
      elif $s == "success" then map(select(.conclusion == "success"))
      else . end
    # bot 噪声过滤: workflow name 命中 BOT_WF_PATTERNS 的 run 不作槽位来源
    | map(select(((.name // .path // "") | test($bot; "i")) | not))
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

pick_jobs_tsv() { # pick_jobs_tsv <jobs-json> <failure|success> — 候选 job "id\tattempt\tname" (保持列表序)
  if [[ $2 == failure ]]; then
    jq -r '.jobs[]? | select(.conclusion == "failure")
      | [(.id|tostring), ((.run_attempt // 1)|tostring), (.name // "?")] | @tsv' <<<"$1"
  else
    jq -r 'if ([.jobs[]? | select(.conclusion == "success")] | length) > 0
      then .jobs[]? | select(.conclusion == "success")
      else (.jobs[0] // empty) end
      | [(.id|tostring), ((.run_attempt // 1)|tostring), (.name // "?")] | @tsv' <<<"$1"
  fi
}

process_run() { # process_run <repo> <dir> <run_id> <wf_name> <failure|success> <created> <max_bytes> <exempt>
  # 返回: 0=新落盘 (配额+1); 1=跳过; 2=早已存在 (已计入初始存量, 勿重复计数)
  local repo=$1 dir=$2 run_id=$3 wf=$4 concl=$5 created=$6 max_b=$7 exempt=$8
  local jobs date base job_id attempt jname
  if run_already_fetched "$dir" "$run_id"; then
    log "  run $run_id 样本已存在 (已计入初始存量)"
    return 2
  fi
  jobs=$(api_get "repos/$repo/actions/runs/$run_id/jobs")
  if [[ -z $jobs ]]; then
    log "  run $run_id jobs 查询失败, 跳过"
    return 1
  fi
  date=${created%%T*}; [[ -n $date ]] || date=unknown
  # 候选 job 顺延语义: 首 job 端点 404/410 或超限 → 同 run 下一个同结论 job
  #   (大仓常见首个 job 即超限 build 日志, 不顺延会饿死槽位)
  while IFS=$'\t' read -r job_id attempt jname; do
    [[ -n $job_id ]] || continue
    jname=$(tr '/ ' '--' <<<"${jname:-unknown}" | cut -c1-30)
    if ((DRY_RUN)); then
      log "  DRY-RUN 选中 run=$run_id job=$job_id (${concl}, jname=$jname, wf=${wf})"
      return 0
    fi
    base="$dir/$date/$run_id-$job_id-$jname-$concl"
    # 嵌套布局 date 目录必须先建 (2026-10-16 事故: 布局迁移加 $date/ 层而未建目录,
    #   curl -o 打开输出文件失败被 || true 吞成 "http=200 空文件", 全候选静默跳过)
    mkdir -p "$dir/$date"
    local rc=0
    download_log "repos/$repo/actions/jobs/$job_id/logs" "$base.log.gz" "$max_b" || rc=$?
    case $rc in
      0)
        # attempt (spec §5 必带, P0-2): jobs 列表自带 run_attempt, 旧 API 形态兜底 1
        [[ $attempt =~ ^[0-9]+$ ]] || attempt=1 # 非数字兜底, 防 --argjson 解析失败炸脚本
        jq -n --arg repo "github.com/$repo" --argjson run_id "$run_id" --argjson job_id "$job_id" \
          --arg concl "$concl" --arg wf "$wf" --arg ca "$created" --arg jn "$jname" \
          --argjson attempt "${attempt:-1}" --argjson ex "$exempt" \
          --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
          '{repo:$repo, run_id:$run_id, job_id:$job_id, conclusion:$concl, workflow_name:$wf,
            created_at:$ca, job_name:$jn, attempt:$attempt, ci:"github", gate_exempt:$ex, fetched_at:$at}' \
          >"$base.log.json"
        log "  已存 run=$run_id job=$job_id (${concl}, $(wc -c <"$base.log.gz")B gz, wf=${wf})"
        return 0;;
      3) continue;; # 尺寸超限 → 下一个候选 job
      *) return 1;;  # 端点不可用 → 弃 run (沿旧语义)
    esac
  done < <(pick_jobs_tsv "$jobs" "$concl")
  log "  run $run_id 无可用 ${concl} job, 跳过"
  return 1
}

fetch_batch() { # fetch_batch <repo> <dir> <concl:failure|success> <want> <max_bytes> <exempt>
  local repo=$1 dir=$2 concl=$3 want=$4 max_b=$5 exempt=$6
  local have line run_id created rest wf
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
      if process_run "$repo" "$dir" "$run_id" "$wf" "$concl" "$created" "$max_b" "$exempt"; then
        have=$((have + 1)) # 仅新落盘才计; 返回 2 (已存在) 已含于初始存量, 不重复计
      fi
    done
  fi
  log "$concl run 最终: $have (目标 $want)"
}

pair_count() { # pair_count <dir> -> 样本对数 (sidecar 计数; 目录缺失 = 0 — dry-run 不建目录)
  local dir=$1
  [[ -d $dir ]] || { echo 0; return; }
  # || true: find 局部失败 (如权限) 不经 pipefail 毒化赋值语句 (set -e 陷阱)
  find "$dir" -name '*.log.json' 2>/dev/null | wc -l || true
}

summarize() { # summarize <tsv行...> — 汇总 + 空语料硬防线; dry-run 只报数不设防线
  local line repo _f _s _m _e dir n total=0 zero=0
  for line in "$@"; do
    IFS=$'\t' read -r repo _f _s _m _e <<<"$line"
    dir="$DATA_DIR/github.com/$repo"
    n=$(pair_count "$dir")
    log "$repo: failure=$(existing_count "$dir" failure) success=$(existing_count "$dir" success) (文件对 $n)"
    total=$((total + 1))
    if ((n == 0)); then zero=$((zero + 1)); fi
  done
  if ((DRY_RUN)); then return 0; fi
  # 空仓硬防线: 全部项目 0 对在场 = 拉取链路整体故障 (认证/网络/API), 拒绝以
  #   "韧性逐条跳过" 之名把空语料合法化 (2026-10-16 事故: GHA per-run token
  #   跨仓 401 → 全跳过 → exit 0 → 空仓被 commit)。已满配额的幂等重跑与部分
  #   项目空 (WARN, 单项目失败不中断) 均不受影响。
  if ((total > 0 && zero == total)); then
    log "ERROR: 全部 $total 项目语料 0 对 — 拉取链路整体故障, 拒绝空仓收场"
    exit 1
  fi
  if ((zero > 0)); then
    log "WARN: $zero/$total 项目 0 对 (逐项目注记见上)"
  fi
}

main() {
  parse_args "$@"
  : "${GITHUB_API_TOKEN:?需要 env GITHUB_API_TOKEN}"
  local -a lines=()
  mapfile -t lines < <(plan_projects)
  if ((${#lines[@]} == 0)); then
    log "ERROR: 项目清单为空 (检查 --projects yaml 语法与 yq 报错)"
    exit 2
  fi
  validate_plan "${lines[@]}"
  if [[ -n $LIMIT ]]; then lines=("${lines[@]:0:LIMIT}"); fi
  local line repo want_f want_s max_b exempt dir
  for line in "${lines[@]}"; do
    IFS=$'\t' read -r repo want_f want_s max_b exempt <<<"$line"
    dir="$DATA_DIR/github.com/$repo" # 嵌套布局 host/owner/repo/<date>/ (可读性优先, 弃 __ 扁平化)
    log "== $repo (槽位 ${want_f}F+${want_s}S, 上限 ${max_b}B, exempt=$exempt) =="
    if ((DRY_RUN == 0)); then mkdir -p "$dir"; fi
    fetch_batch "$repo" "$dir" failure "$want_f" "$max_b" "$exempt"
    fetch_batch "$repo" "$dir" success "$want_s" "$max_b" "$exempt"
  done
  log "== 汇总 =="
  summarize "${lines[@]}"
}

main "$@"

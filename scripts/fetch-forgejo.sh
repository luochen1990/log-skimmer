#!/usr/bin/env bash
# fetch-forgejo.sh — Forgejo 侧语料现场拉取 (私有内网实例, 数据永不出内网)
#
# 职责边界: 按 data/forgejo-runs.json 固定清单 (run/job id + 不可变元数据固化) 从
#   forgejo API 拉取 job 日志到语料根的 git.lambda.lc__lc-studio__nixos/ — 与
#   fetch-corpus.sh (GitHub 公开语料仓) 组成 L0 门禁的双源语料。
# 隐私红线: 本仓日志含内网拓扑 (域名/网段/主机名), **禁止进入任何外网仓**
#   (事故记录: 2026-10-16 曾推 GitHub 语料仓 ~1h, 已重写历史并重建; 见 AGENTS.md)。
# 幂等: 日志已存在跳过; 清单变更 (PR 更新 run_id) 后增量拉取。
# 稳定性: run 日志不可变, 固定清单 = 稳定语料; 换样本走 PR 改清单。
# 认证: FORGEJO_API_TOKEN (必需 — CI 的自动 GITHUB_TOKEN 是 PR 所在仓 scoped,
#   读不了 nixos 仓; 需专门 secret, 读权限 PAT)。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DL="${CORPUS_DIR:-$ROOT/data/logs}"
MANIFEST="$ROOT/data/forgejo-runs.json"
API="https://git.lambda.lc/api/v1"

log() { printf '[fetch-forgejo %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }

need_token=0
[ -n "${FORGEJO_API_TOKEN:-}" ] || need_token=1
[ -f "$MANIFEST" ] || { echo "清单不存在: $MANIFEST" >&2; exit 1; }

OUT="$DL/git.lambda.lc/lc-studio/nixos"
n=0
while IFS=$'\t' read -r run_id job_id; do
  [ -n "$run_id" ] || continue
  # 新布局路径从清单固化元数据派生 (created_at→日期, job_name, conclusion)
  m=$(jq -c ".[] | select(.run_id==$run_id)" "$MANIFEST")
  dest_dir="$OUT/$(jq -r .created_at <<<"$m" | cut -dT -f1)"
  base="${run_id}-${job_id}-$(jq -r .job_name <<<"$m")-$(jq -r .conclusion <<<"$m")"
  log_file="$dest_dir/$base.log"
  if [ -s "$log_file" ]; then n=$((n+1)); continue; fi
  # 惰性凭据: 全在场幂等跳过场景零凭据可跑; 真拉时才要求 token
  if [ "$need_token" = 1 ]; then
    echo "缺 FORGEJO_API_TOKEN (读 lc-studio/nixos actions 的 PAT; 已在场 $n 对跳过拉取)" >&2
    exit 1
  fi
  mkdir -p "$dest_dir"
  # 日志本体 (raw 端点, 纯文本); 404 = 日志已被清理, 属清单腐坏信号 → 硬错提示换样本
  code=$(curl -sfSL --max-time 90 --retry 2 \
    -H "Authorization: token $FORGEJO_API_TOKEN" \
    -o "$log_file.tmp" -w '%{http_code}' \
    "$API/repos/lc-studio/nixos/actions/jobs/$job_id/logs" || echo fail)
  if [ "$code" != "200" ]; then
    rm -f "$log_file.tmp"
    echo "拉取失败 run=$run_id job=$job_id (http=$code): 日志被清理或权限不足 — 若已过期, PR 更新清单换样本" >&2
    exit 1
  fi
  mv "$log_file.tmp" "$log_file"
  # sidecar: 清单固化不可变元数据, 仅 fetched_at 动态
  # -n: 无输入求值 — 不带输入源的 jq 会偷吃 while 循环的 stdin (manifest 剩余行),
  #     循环首条即 EOF 提前结束 (run 19465 形态: 拉到 1 对后 "在场数与清单不符")
  jq -n "$m + {fetched_at: \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}" > "$dest_dir/$base.log.json"
  n=$((n+1))
done < <(jq -r '.[] | [.run_id, .job_id] | @tsv' "$MANIFEST")

log "在场 $n 对 (清单 $(jq length "$MANIFEST") 条)"
[ "$n" -eq "$(jq length "$MANIFEST")" ] || { echo "在场数与清单不符" >&2; exit 1; }

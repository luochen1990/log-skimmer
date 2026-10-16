# 职责: 开发/CI 任务入口 — org 标准 ci-merge P0 契约 (零参数/阻塞级/幂等/快步骤先行) 的宿主;
# 门禁链细目见 engine/src/l0.rs 头注释与 docs/rule-format.md §4 验收线。

# 本地快速循环: fmt/clippy/单测 (devShell 内零参数; 不含 L0 全语料门禁)
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# 规则加载回归 (engine/tests/rules_load.rs): 防规则语法漂移进仓。
# 已含于 check 的 workspace test, 单列醒目 — 规则文件是仓内高频变更面
rules-load:
    cargo test --test rules_load

# L0 门禁: 全语料回归, 任一违例即红 (FN=0 / FP 率 ≤2% / 边界违例=0 / 零块=0 /
# 压缩率双测防退化 — master 引擎 vs PR 引擎 × 同语料同规则, 纯引擎变量归因)。
# 判定口径与断言 SSOT: engine/src/l0.rs (log-skimmer l0 子命令); 编排 scripts/l0-run.sh。
l0-gate:
    bash scripts/l0-run.sh

# L0 人类可读报表 (非门禁, 开发参考): summary JSON 直出
l0-stats:
    cargo build --release -q
    ./target/release/log-skimmer l0 --corpus data/logs --rules data/rules --emit-summary

# 语料获取 (幂等, 双源): GitHub 公开语料仓 (GHA 57 对) + forgejo 内网现场拉取
# (固定清单 10 对, 隐私红线: 数据不出内网; 全在场时零凭据)
corpus-fetch:
    bash scripts/fetch-corpus.sh
    bash scripts/fetch-forgejo.sh

# 规则集获取 (幂等): log-skimmer-rules 仓 (public, 社区维护) → data/rules
rules-fetch:
    bash scripts/fetch-rules.sh

# 发布面同步 (手动节奏, 需求方裁决): master → GitHub luochen1990/log-skimmer。
# 发布后 rules 仓 lint CI 的引擎引用自动取新 (发布面空壳期 lint 显式降级)。
publish:
    #!/usr/bin/env bash
    set -euo pipefail
    remote_url="https://github.com/luochen1990/log-skimmer.git"
    git remote get-url github >/dev/null 2>&1 || git remote add github "$remote_url"
    git push --force-with-lease github master:master
    echo "发布面已同步: github.com/luochen1990/log-skimmer master @ $(git rev-parse --short master)"

# ci-periodic — org 标准 P2 周期兜底契约 (非超集/阻塞级/幂等): 内网现场语料补抓
# (fetch-forgejo 是内网 API 调用, 零出网流量) + 语料仓新鲜度哨兵 (GHA weekly 自刷
# 挂掉时暴露; 刷新闭环全在 GitHub 机房, 本实例零参与)
ci-periodic: corpus-fetch
    # 语料仓 HEAD 距今 >8 天判红 (weekly 刷新 + 余量; 单行: just 各行独立 shell 不共享变量); 自愈: 语料仓手动 dispatch
    age=$(( $(date +%s) - $(date -d "$(git -C data/logs log -1 --format=%cI)" +%s) )); [ "$age" -lt $((8*86400)) ] || { echo "语料仓陈旧 ($((age/86400)) 天未刷新, GHA 刷新链疑似故障"; exit 1; }; echo "语料新鲜度 OK ($((age/86400)) 天)"

# ci-merge — org 标准 CI P0 合入门禁契约:
#   阻塞级 (失败即红, 阻塞 PR 合入) / 零参数 / 幂等 / 快步骤先行 (fmt→clippy→test→L0)。
# CI runner (vm-nix) 契约: 预装工具链直用, 禁 nix develop (VM ro store + temproots
# 无写权限, nix 求值必炸 — forgejo-runner-vm config 头注释; secret-guard 同模式)。
# 本地开发: 系统无 rust 时 `nix develop -c just check` 进 devShell (flake.nix)。
ci-merge: _ci-merge

_ci-merge: check corpus-fetch rules-fetch l0-gate

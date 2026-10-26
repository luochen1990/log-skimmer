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

# wasm 形态对账 (wasm 交付形态 log-skimmer-wasm): 构建 wasm32 产物 + 原生 CLI,
# 同 fixture 样本全量对账 (blocks 逐项 / stats / input/output_lines, 不一致即红)。
# 独立于 check, 理由: ① check 被 ci-merge 消费, 而 forgejo runner 契约 = 预装
# 工具链直用 (无 wasm32 链接配置, 且禁 nix develop — AGENTS「CI」节), 并入即给
# CI 契约引入新环境前置; ② check 链原则 = 快, wasm32 是另一目标的全程编译。
# wasm crate 源码已受 check 覆盖 (clippy/test 按宿主目标编译); 本 recipe 验证
# 的增量 = wasm32 实际编译链接 + ABI 端到端。产物级 CI 归未来 web 仓。
# 依赖 devShell 的 lld + nodejs (flake.nix)。改 log-skimmer-wasm/ 或 engine
# 装载语义时本地跑。
wasm-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release -q -p log-skimmer-wasm --target wasm32-unknown-unknown
    cargo build --release -q -p log-skimmer
    native="$(mktemp /tmp/opencode/wasm-reconcile.XXXXXX.json)"
    trap 'rm -f "$native"' EXIT
    ./target/release/log-skimmer skim log-skimmer-wasm/tests/fixtures/ci-sample.log \
        --rules log-skimmer-wasm/tests/fixtures/rules --json > "$native"
    node log-skimmer-wasm/reconcile.mjs log-skimmer-wasm/tests/fixtures/rules \
        log-skimmer-wasm/tests/fixtures/ci-sample.log --native "$native"

# L0 门禁: 全语料回归, 任一违例即红 (FN=0 / FP 率 ≤2% / 边界违例=0 / 零块=0 /
# 压缩率双测防退化 — master 引擎 vs PR 引擎 × 同语料同规则, 纯引擎变量归因;
# FN/FP/零块对 gated 域 — sidecar gate_exempt 仓豁免过渡, spec §4)。
# 判定口径与断言 SSOT: engine/src/l0.rs (log-skimmer l0 子命令); 编排 scripts/l0-run.sh。
l0-gate:
    bash scripts/l0-run.sh

# L0 人类可读报表 (非门禁, 开发参考): summary JSON 直出
l0-stats:
    cargo build --release -q
    ./target/release/log-skimmer l0 --corpus data/logs --rules data/rules --emit-summary

# 语料获取 (幂等, 双源): GitHub 公开语料仓 (GHA 57 对) + forgejo 内网现场拉取
# (固定清单 10 对, 隐私红线: 数据不出内网; 全在场时零凭据)。
corpus-fetch:
    bash scripts/fetch-corpus.sh
    bash scripts/fetch-forgejo.sh

# 规则集获取 (幂等): log-skimmer-rules 仓 (public, 社区维护) → data/rules
rules-fetch:
    bash scripts/fetch-rules.sh

# 凭据形态门禁 (hygiene + data/secret-patterns.txt, 一行一正则): 同树镜像模型下
# 本仓树即公开树, 凭据误提交零容忍 (关键隐私红线 — 唯一机械防线)。
# 扫 tracked 文件的工作区版本 (tar 清单直取, 非 git archive HEAD): 覆盖未提交
# 修改 — 提交前跑即拦 (archive HEAD 有未提交盲区, run 20826 实证: fixture 假
# token 本地漏检、CI 红); 亦避开未跟踪面 (target/ 与 data/logs 语料 clone 的
# token 形态假阳性)。
secret-scan:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp="$(mktemp -d "${TMPDIR:-/tmp}/log-skimmer-secret-scan.XXXXXX")"
    trap 'rm -rf "$tmp"' EXIT
    tar -C . --null -T <(git ls-files -z) -cf - | tar -C "$tmp" -xf -
    cargo build --release -q -p log-skimmer
    "${CARGO_TARGET_DIR:-target}/release/log-skimmer" hygiene --patterns data/secret-patterns.txt "$tmp"

# 发布面同步 (同树镜像, 需求方裁决 2026-10-20): master + tags → GitHub
# luochen1990/log-skimmer — 两边同 SHA 同历史同 tags。
# 快照式时代 (issue #18 → #84 tag 残留事故) 已退役: 差异即复杂度; 隐私治理改
# 源头脱敏 + 凭据形态门禁 (secret-scan, 上方), 见 AGENTS「公开镜像纪律」。
# 在 master 上运行; 首次镜像 push 需一次性 force 覆盖旧 orphan 快照
# (两边无共同祖先): git push --force github master --tags — 之后恒 fast-forward。
publish: secret-scan
    #!/usr/bin/env bash
    set -euo pipefail
    git push https://github.com/luochen1990/log-skimmer.git master --tags
    echo "发布面已镜像: github.com/luochen1990/log-skimmer master+tags ← $(git rev-parse --short master)"

# ci-periodic — org 标准 P2 周期兜底契约 (非超集/阻塞级/幂等): 内网现场语料补抓
# (fetch-forgejo 是内网 API 调用, 零出网流量) + 语料仓新鲜度哨兵 (GHA weekly 自刷
# 挂掉时暴露; 刷新闭环全在 GitHub 机房, 本实例零参与)
ci-periodic: corpus-fetch
    # 语料仓 HEAD 距今 >8 天判红 (weekly 刷新 + 余量; 单行: just 各行独立 shell 不共享变量); 自愈: 语料仓手动 dispatch
    age=$(( $(date +%s) - $(date -d "$(git -C data/logs log -1 --format=%cI)" +%s) )); [ "$age" -lt $((8*86400)) ] || { echo "语料仓陈旧 ($((age/86400)) 天未刷新, GHA 刷新链疑似故障"; exit 1; }; echo "语料新鲜度 OK ($((age/86400)) 天)"

# ci-merge — org 标准 CI P0 合入门禁契约:
#   阻塞级 (失败即红, 阻塞 PR 合入) / 零参数 / 幂等 / 快步骤先行 (secret-scan→fmt→clippy→test→L0,
#   check 内部步骤顺序不变)。
#   secret-scan 前置: 凭据形态门禁 (关键隐私红线 — 同树镜像模型下 PR 树即将全量公开)。
#   rules-fetch 前置于 check: rules_load 测试消费 RULES_DIR env (CI job env 穿透指向
#   持久缓存, check 先行则首跑缓存未建必红 — PR #40 run 19848 实证), 规则必须在场;
#   这也是该 env 覆盖特性的设计意图 — CI 中 rules_load 升级为真实规则集回归
#   (本地无 env 走 fixture, 零影响)。
# CI runner 契约: 预装工具链直用, 禁 nix develop (VM ro store + temproots
# 无写权限, nix 求值必炸 — forgejo-runner-vm config 头注释; secret-guard 同模式)。
# 本地开发: 系统无 rust 时 `nix develop -c just check` 进 devShell (flake.nix)。
ci-merge: _ci-merge

_ci-merge: rules-fetch secret-scan check corpus-fetch l0-gate

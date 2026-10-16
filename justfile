# 职责: 开发/CI 任务入口 — org 标准 ci-merge P0 契约 (零参数/阻塞级/幂等/快步骤先行) 的宿主;
# 门禁链细目见 scripts/l0-gate.sh 头注释与 docs/rule-format.md §4 验收线。

# 本地快速循环: fmt/clippy/单测 (devShell 内零参数; 不含 L0 全语料门禁)
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# 规则加载回归 (engine/tests/rules_load.rs): 防规则语法漂移进仓。
# 已含于 check 的 workspace test, 单列醒目 — 规则文件是仓内高频变更面
rules-load:
    cargo test --test rules_load

# L0 门禁: 全语料回归, 任一违例即红 (FN=0 / FP 率 ≤2% / 边界违例=0 / 零块=0 / 基线防退化)。
# 最慢步骤, 契约链末位; collector 自动定位二进制 (CI 持久卷 CARGO_TARGET_DIR 亦覆盖)
l0-gate:
    cargo build --release
    bash scripts/l0-gate.sh

# L0 人类可读报表 (非门禁, 开发参考; rg 基线对照)
l0-stats:
    bash scripts/l0-stats.sh

# 语料获取 (幂等, 双源): GitHub 公开语料仓 (GHA 57 对) + forgejo 内网现场拉取
# (固定清单 10 对, 隐私红线: 数据不出内网; 全在场时零凭据)
corpus-fetch:
    bash scripts/fetch-corpus.sh
    bash scripts/fetch-forgejo.sh

# 判定器镜像同步 (P2 周期档载体): 主仓 engine/rules/L0 脚本 → 语料仓镜像目录,
# 有差则推送 + dispatch 触发语料仓 GHA 即时刷新 (新引擎 × 当前语料重算基线)。
# 语料槽位滚动重拉在语料仓 GHA 侧自执行 (GitHub 机房闭环) — 语料字节不走内网出口
# (省外网流量); 本 recipe 只送文本镜像 (~150KB 级, 低频)。
# 需 CORPUS_GH_TOKEN; 语料数据变更纪律: 一律改语料仓工作副本并提交, 主仓 data/logs 只读消费
corpus-sync:
    bash scripts/corpus-sync.sh

# ci-periodic — org 标准 P2 周期兜底契约 (非超集/阻塞级/幂等): 判定器镜像同步 +
# 内网现场语料补抓 (fetch-forgejo 是内网 API 调用, 零出网流量; 新语料的 FN 信号由
# 主仓下一 PR 的 l0-gate 暴露, 归因明确)
ci-periodic: corpus-sync
    bash scripts/fetch-forgejo.sh

# ci-merge — org 标准 CI P0 合入门禁契约:
#   阻塞级 (失败即红, 阻塞 PR 合入) / 零参数 / 幂等 / 快步骤先行 (fmt→clippy→test→L0)。
# CI runner (vm-nix) 契约: 预装工具链直用, 禁 nix develop (VM ro store + temproots
# 无写权限, nix 求值必炸 — forgejo-runner-vm config 头注释; secret-guard 同模式)。
# 本地开发: 系统无 rust 时 `nix develop -c just check` 进 devShell (flake.nix)。
ci-merge: _ci-merge

_ci-merge: check corpus-fetch l0-gate

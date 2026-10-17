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
# (固定清单 10 对, 隐私红线: 数据不出内网; 全在场时零凭据)。
# fetch-forgejo 为内网专用件 (不入发布面, issue #18): 在场才拉 — 发布面克隆跑本 recipe 优雅降级。
corpus-fetch:
    bash scripts/fetch-corpus.sh
    if [ -f scripts/fetch-forgejo.sh ]; then bash scripts/fetch-forgejo.sh; else echo "跳过内网语料拉取 (scripts/fetch-forgejo.sh 不在场, 发布面形态)"; fi

# 规则集获取 (幂等): log-skimmer-rules 仓 (public, 社区维护) → data/rules
rules-fetch:
    bash scripts/fetch-rules.sh

# 发布树暂存目录默认值 (publish-stage/publish 共用 SSOT; env PUBLISH_DIR 的覆盖
# 逻辑在各 recipe bash 侧 `${PUBLISH_DIR:-...}` 运行期展开 — env 值不经 just 文本插值)
publish_dir := '/tmp/opencode/log-skimmer-publish'

# 发布树构造 + 卫生检查 (publish 必经前段; 可独立运行验证发布面形态)。
# master git 树 → 临时目录 − 内网专用件 → `log-skimmer hygiene` 扫描 (命中即 fail)。
# 排除清单 SSOT 在此; 内网模式清单 SSOT 在 data/intranet-patterns.txt (同为内网专用件
# — 清单本身是内网端点情报, 逻辑与机密分离: engine/src/hygiene.rs 只随公开面逻辑零字面量)。
publish-stage:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="${PUBLISH_DIR:-{{publish_dir}}}" # env 由 bash 运行期展开; 插值恒为无特殊字符常量
    # 机械断言: master 无 tracked-but-ignored 文件 — .gitignore pattern 失效 (target*/
    # 行内注释事故, 已发生两次: 38MB 误提交 + 本次 target/ 入 index) 会破下方发布树不变量
    if git ls-files -i -c --exclude-standard | grep -q .; then
        echo "tracked-but-ignored 文件在场 (.gitignore pattern 失效?), 拒绝发布:" >&2
        git ls-files -i -c --exclude-standard >&2
        exit 1
    fi
    rm -rf "$dir" && mkdir -p "$dir"
    git archive master | tar -x -C "$dir"
    # 内网专用件排除清单 (issue #18; 每项理由):
    #   AGENTS.md                — 内网维护者文档 (域名/网段/主机名/runner 契约/凭据叙述)
    #   scripts/fetch-forgejo.sh — 内网语料拉取 (域名/org/API 路径, issue 主件)
    #   data/forgejo-runs.json   — 内网 run 固定清单 (org/repo + run 元数据)
    #   data/intranet-patterns.txt — 卫生检查内网模式清单 (内网端点情报本体)
    #   docs/category-survey.md  — 立项调研 (内网 issue 引用 + 内网网段示例)
    #   .forgejo/                — 内网 CI 编排 (runner 标签/secret 名/持久卷路径)
    #   poc-dag.md               — POC 过程工件 (自述不入正式仓, 含内网建仓叙述)
    rm -rf "$dir/AGENTS.md" "$dir/scripts/fetch-forgejo.sh" "$dir/data/forgejo-runs.json" \
        "$dir/data/intranet-patterns.txt" "$dir/docs/category-survey.md" \
        "$dir/.forgejo" "$dir/poc-dag.md"
    # 不变量: 发布树文件集 ≡ master git 树 − 上述排除清单 (tracked-but-ignored
    # 断言已在上方把守前提)
    cargo build --release -q -p log-skimmer
    "${CARGO_TARGET_DIR:-target}/release/log-skimmer" hygiene --patterns data/intranet-patterns.txt "$dir"
    echo "发布树就绪: $dir (hygiene 通过)"

# 发布面同步 (手动节奏, 需求方裁决): master → GitHub luochen1990/log-skimmer。
# 快照式 (orphan 单 commit): 发布面无累计历史 — 内网专用件不仅当前树不入,
# 历史亦不残留 (issue #18: 此前整树镜像把内网端点连同历史带了出去)。
# 排除与卫生检查必经 publish-stage (机械防线); 发布后 rules 仓 lint CI 的
# 引擎引用自动取新 (发布面空壳期 lint 显式降级)。
publish: publish-stage
    #!/usr/bin/env bash
    set -euo pipefail
    sha="$(git rev-parse --short master)"
    dir="${PUBLISH_DIR:-{{publish_dir}}}"
    remote_url="https://github.com/luochen1990/log-skimmer.git"
    cd "$dir"
    git init -q # 全新仓 (publish-stage 刚 rm -rf 重建): orphan 首提交, 无历史残留
    git add -A
    git -c user.name="log-skimmer publish" -c user.email="publish@localhost" \
        commit -qm "publish: snapshot of master@$sha (intranet-only files excluded, hygiene-checked)"
    git remote add github "$remote_url"
    git fetch -q github master || true # lease 基准 (首次 publish 前远端可无 master)
    expected="$(git rev-parse --verify -q github/master || true)"
    if [ -n "$expected" ]; then
        git push --force-with-lease=refs/heads/master:"$expected" github HEAD:master
    else
        git push github HEAD:master
    fi
    echo "发布面已同步: github.com/luochen1990/log-skimmer master ← master@$sha 快照"

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

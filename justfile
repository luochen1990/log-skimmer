# 职责: 开发/CI 任务入口 (org ci-merge 契约的宿主; 各 recipe 快步骤先行)

# 本地全量检查 (等价 ci-merge 契约的核心链; 在仓库根零参数调用)
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# ci-merge — org 标准 CI P0 合入门禁契约 (零参数/阻塞级/幂等/快步骤先行)
ci-merge: check

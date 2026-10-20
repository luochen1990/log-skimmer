//! 发布面 tag 卫生检查 (`log-skimmer tag-guard` 子命令, publish 机械防线之一)。
//!
//! # 职责边界
//! 校验远端 `refs/tags/*` 的每个 tag 指向**合法快照 commit**:
//! (a) orphan — 零 parent (发布面无累计历史, issue #18: 排除件连同历史不残留);
//! (b) 树内不含内网专用件 — 排除清单 SSOT `data/publish-excludes.txt`
//!     (与 justfile publish-stage 的发布树构造 rm 共源)。
//!
//! # 契约 (2026-10-20 tag 残留事故补口)
//! 快照式 publish 只 force push master, 不触碰 tags — 事故形态: 10-17 的
//! v0.1.0 tag 指向 dev 面提交, 45-commit 内网历史链经 tag 在公开面存活 2 天。
//! 本模块把 "publish 的 ref 管理范围必须覆盖 refs/tags/*" 固化为机械断言:
//! publish 流程必经本检查, 违规 tag 即 fail (列名 + 修复指引), 拒绝静默放过。
//!
//! # 分层
//! - 判定 ([audit]) 与 ls-remote 解析 ([parse_ls_remote]) 是纯函数 — 表驱动单测;
//! - git 采集 (ls-remote/fetch/rev-list/ls-tree) 是 subprocess 薄封装 —
//!   e2e 用本地 bare fixture 仓覆盖 (零网络, fixture 零真实内网内容)。

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// 排除清单装载 (一行一路径前缀, # 注释; noun=条目) — 复用 hygiene 行式装载器,
/// 与 publish-stage 的 rm 口径同源 (SSOT data/publish-excludes.txt)。
/// 路径条目专属校验: 拒绝内部空白 (rm 管道按行消费, 含空白路径不在支持面;
/// 此校验不进共享 load_lines — hygiene 模式是子串语义, 空白有意义)。
pub fn load_excludes(path: &Path) -> Result<Vec<String>, String> {
    let entries = crate::hygiene::load_lines(path, "条目")?;
    if let Some(bad) = entries.iter().find(|e| e.chars().any(char::is_whitespace)) {
        return Err(format!(
            "{}: 条目含内部空白 — {bad:?} (路径条目不支持空白)",
            path.display()
        ));
    }
    Ok(entries)
}

/// 单 tag 审计结果 (violation 判定见 [TagAudit::ok])。
#[derive(Debug)]
pub struct TagAudit {
    /// ref 短名 (如 `v0.1.0`, 不含 `refs/tags/` 前缀)
    pub tag: String,
    /// tag 指向对象非 commit (blob/tree — 畸形 tag 面) 时的对象类型
    pub malformed: Option<String>,
    /// tag 指向 commit 的 parent 数 (0 = orphan 快照)
    pub parents: usize,
    /// 树内命中排除清单的路径 (排序序, 报告稳定可 diff)
    pub hits: Vec<String>,
}

impl TagAudit {
    /// 合法快照 = 非畸形 且 orphan 且树零命中排除件。
    pub fn ok(&self) -> bool {
        self.malformed.is_none() && self.parents == 0 && self.hits.is_empty()
    }
}

/// 判定纯函数: tag 指向的 (parents, 树路径集) 对照排除清单审计。
/// 匹配语义: 路径等于条目, 或以条目为前缀 (目录条目尾斜杠保证组件边界 —
/// `data/` 命中 `data/x` 不命中 `database/x`; 文件条目前缀轻微 over-match
/// 如 `AGENTS.md.bak`, 方向上多报不漏报, 可接受)。
pub fn audit(tag: &str, parents: usize, paths: &[String], excludes: &[String]) -> TagAudit {
    let mut hits: Vec<String> = paths
        .iter()
        .filter(|p| {
            excludes
                .iter()
                .any(|e| p.as_str() == e || p.starts_with(e.as_str()))
        })
        .cloned()
        .collect();
    hits.sort_unstable();
    TagAudit {
        tag: tag.to_string(),
        malformed: None,
        parents,
        hits,
    }
}

/// 解析 `git ls-remote --tags` 输出 (纯函数)。
/// annotated tag 出双行 (tag 对象 + `^{}` peel 行) — peel 行指向 commit, 覆盖本体;
/// 轻量 tag 单行, 本体即 commit。非 tag ref 行忽略。返回 tag 名排序序。
pub fn parse_ls_remote(raw: &str) -> Vec<(String, String)> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for line in raw.lines() {
        let Some((sha, r)) = line.split_once('\t') else {
            continue;
        };
        let Some(name) = r.strip_prefix("refs/tags/") else {
            continue;
        };
        if let Some(peeled) = name.strip_suffix("^{}") {
            map.insert(peeled.to_string(), sha.to_string());
        } else {
            map.entry(name.to_string())
                .or_insert_with(|| sha.to_string());
        }
    }
    map.into_iter().collect()
}

/// tag 面总检 (CLI 入口): 固定 scratch ctx 后委托 [guard_at]。
/// Err = 采集层失败 (远端不可达 / git 缺失), CLI 映射 exit 2。
/// 固定 ctx 名 = 单写者假设 (publish 手动节奏成立; 并发 publish 会互踩 scratch
/// 对象库 — 残留 refs 不影响判定正确性, 审计由当次 ls-remote 驱动)。
pub fn guard(remote: &str, excludes: &[String]) -> Result<Vec<TagAudit>, String> {
    guard_at(
        &std::env::temp_dir().join("log-skimmer-tag-guard-ctx"),
        remote,
        excludes,
    )
}

/// 采集得到的单 tag 事实 (畸形 tag 与 commit 元数据互斥)。
enum TagFacts {
    /// tag 指向非 commit 对象 (对象类型为 payload)
    Malformed(String),
    /// tag 指向 commit: parent 数 + 树路径集
    Commit { parents: usize, paths: Vec<String> },
}

/// tag 面总检核心 (测试入口: ctx 由调用方给定, e2e 用例间隔离)。
/// ctx 为任意可创建目录 — 采集所需 git 上下文 (init 幂等) 自备:
/// ls-remote 虽不依赖本地仓, fetch 须有本地对象库。
/// Ok = 审计全表 (含合规项, 报告用), 违规判定在消费方; Err = 采集失败。
/// 单 tag 畸形仅折损该 tag 的事实 (Malformed 分类), 不中断其余 tag 审计 —
/// 事故现场须完整报告, 不因一个畸形 tag 丢掉其余违规。
pub fn guard_at(ctx: &Path, remote: &str, excludes: &[String]) -> Result<Vec<TagAudit>, String> {
    std::fs::create_dir_all(ctx).map_err(|e| format!("ctx 目录创建失败 {}: {e}", ctx.display()))?;
    git(ctx, &["init", "--quiet"])?;
    let raw = git(ctx, &["ls-remote", "--tags", remote])?;
    let mut audits = Vec::new();
    for (name, sha) in parse_ls_remote(&raw) {
        audits.push(match inspect_tag(ctx, remote, &name, &sha)? {
            TagFacts::Malformed(kind) => TagAudit {
                tag: name,
                malformed: Some(kind),
                parents: 0,
                hits: Vec::new(),
            },
            TagFacts::Commit { parents, paths } => audit(&name, parents, &paths, excludes),
        });
    }
    Ok(audits)
}

/// 采集单 tag 指向对象的事实。fetch 走 tag refspec (服务器普遍支持,
/// 不依赖 allow-any-sha fetch); cat-file -t 前置判型 — 非 commit 即畸形
/// (violation, 非采集层失败): 对应 tag 面被恶意/误构对象占用。
fn inspect_tag(ctx: &Path, remote: &str, tag: &str, sha: &str) -> Result<TagFacts, String> {
    git(
        ctx,
        &[
            "fetch",
            "--quiet",
            remote,
            &format!("+refs/tags/{tag}:refs/tags/{tag}"),
        ],
    )?;
    let kind = git(ctx, &["cat-file", "-t", sha])?;
    let kind = kind.trim();
    if kind != "commit" {
        return Ok(TagFacts::Malformed(kind.to_string()));
    }
    // rev-list --parents -n1 输出 "sha [p1 p2 ...]": 词数-1 即 parent 数
    let parents_raw = git(ctx, &["rev-list", "--parents", "-n", "1", sha])?;
    let parents = parents_raw.split_whitespace().count().saturating_sub(1);
    let tree_raw = git(ctx, &["ls-tree", "-r", "--name-only", sha])?;
    Ok(TagFacts::Commit {
        parents,
        paths: tree_raw.lines().map(str::to_string).collect(),
    })
}

/// git 子进程薄封装 (cwd = ctx; stderr 并入 Err 定位)。
fn git(ctx: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(ctx)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

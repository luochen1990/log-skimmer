//! tag-guard 测试 — 判定纯函数表驱动 + 本地 bare fixture 仓 e2e (零网络)。
//! fixture 的"内网专用件"命中靠路径名本身 (AGENTS.md 等), 内容零真实内网情报
//! (防线代码与机密分离约束, 与 hygiene 测试同款)。
//! 覆盖: audit 三轴 (orphan / 精确文件 / 目录前缀与组件边界不误伤) 与
//! malformed 独立违规轴 / ls-remote 解析 (peel 行优先 / 非 tag ref 忽略 / 空输出) /
//! e2e (合法快照通过 / 历史链拒 / 含排除件拒 / annotated peel 链路双形态 /
//! 混合面聚合排序 / blob 畸形违规且不中断其余审计 / 空 tag 面通过 /
//! 远端不可达采集错) / 装载回归 (生产清单 + 内部空白拒 + 发布面 skip)。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use log_skimmer::tag_guard::{audit, guard_at, load_excludes, parse_ls_remote};

/// 测试用排除清单 (真实全量清单的装载回归另见 production_excludes_file_loads)。
const EX: &[&str] = &["AGENTS.md", "data/", ".forgejo/", "scripts/l0-run.sh"];

fn excludes() -> Vec<String> {
    EX.iter().map(|s| s.to_string()).collect()
}

// ---------- 纯函数: audit ----------

#[test]
fn audit_passes_orphan_with_clean_tree() {
    let a = audit(
        "v1",
        0,
        &["README.md".into(), "engine/src/lib.rs".into()],
        &excludes(),
    );
    assert!(a.ok(), "orphan + 零命中 = 合法快照: {a:?}");
}

#[test]
fn audit_rejects_parented_commit_even_with_clean_tree() {
    // has-parent 是独立违规轴: 树再干净, 历史链经 tag 可达即违规 (issue #18)
    let a = audit("v1", 1, &["README.md".into()], &excludes());
    assert!(!a.ok(), "非 orphan 不得放行: {a:?}");
}

#[test]
fn malformed_tag_is_violation_even_without_other_axes() {
    // 畸形轴独立成立: 指向非 commit 对象即违规, 与 parent/树无关
    let a = log_skimmer::tag_guard::TagAudit {
        tag: "v9".into(),
        malformed: Some("blob".into()),
        parents: 0,
        hits: Vec::new(),
    };
    assert!(!a.ok(), "{a:?}");
}

#[test]
fn audit_hits_exact_file_and_dir_prefix_without_false_sibling() {
    let paths: Vec<String> = [
        "AGENTS.md",
        "data/forgejo-runs.json",
        "database/x", // data/ 的同名前缀兄弟目录 — 组件边界不得误伤
        ".forgejo/workflows/ci.yml",
        "scripts/l0-run.sh",
        "scripts/fetch-logs.sh", // 同目录非清单件 — 不得连带
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let a = audit("v1", 0, &paths, &excludes());
    assert_eq!(
        a.hits,
        [
            ".forgejo/workflows/ci.yml",
            "AGENTS.md",
            "data/forgejo-runs.json",
            "scripts/l0-run.sh",
        ],
        "命中 = 精确文件 + 目录前缀内, 且按路径排序: {a:?}"
    );
}

// ---------- 纯函数: parse_ls_remote ----------

#[test]
fn parse_prefers_peel_line_and_ignores_non_tags() {
    let raw = "shaM\trefs/heads/master\n\
               shaT\trefs/tags/v0.1.0\n\
               shaC\trefs/tags/v0.1.0^{}\n\
               shaL\trefs/tags/v0.2.0\n";
    assert_eq!(
        parse_ls_remote(raw),
        vec![
            ("v0.1.0".to_string(), "shaC".to_string()), // peel 行 (commit) 覆盖 tag 对象行
            ("v0.2.0".to_string(), "shaL".to_string()), // 轻量 tag: 本体即 commit
        ]
    );
}

#[test]
fn parse_empty_and_garbage_yield_empty() {
    assert!(parse_ls_remote("").is_empty());
    assert!(parse_ls_remote("garbage\nno\tref\n\t\nsha\trefs/heads/x").is_empty());
}

// ---------- e2e: 本地 bare fixture 仓 (零网络) ----------

/// 每用例独立 scratch 目录 (temp_dir 下前缀隔离; 重复运行幂等覆盖)。
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("log-skimmer-tag-guard-{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// git 薄封装 (fixture 构造失败 = 测试基础设施错, 直接 panic)。
/// gpgsign/tag.gpgsign: 隔离宿主全局签名配置 (硬件密钥不在场时 fixture 无法
/// 建 commit/tag); user.*: fixture 提交身份, 不依赖宿主全局 config。
fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 造远端 bare 仓: files 全部 commit 后, 按 orphan 取向打一个 tag。
/// orphan=true: tag → commit-tree 无 parent 快照; false: tag → 第二个普通
/// commit (带 parent, 即事故形态: dev 历史链上的提交)。
/// annotated=true: annotated tag (peel 行链路 — 发布面 Release UI 的默认形态);
/// false: 轻量 tag。
/// 返回 (guard 用的独立 ctx 目录, bare 远端路径, work 仓 — 供用例继续叠 tag)。
fn fixture_remote(
    name: &str,
    files: &[(&str, &str)],
    orphan: bool,
    tag: Option<&str>,
    annotated: bool,
) -> (PathBuf, PathBuf, PathBuf) {
    let d = scratch(name);
    let bare = d.join("remote.git");
    let work = d.join("work");
    fs::create_dir_all(&work).unwrap();
    sh(&d, &["init", "--bare", "-q", "remote.git"]);
    sh(&work, &["init", "-q"]);
    for (rel, content) in files {
        let p = work.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    sh(&work, &["add", "-A"]);
    sh(&work, &["commit", "-qm", "base"]);
    if let Some(tag) = tag {
        push_tag(
            &work,
            &bare,
            tag,
            &snapshot_target(&work, orphan),
            annotated,
        );
    }
    (d.join("ctx"), bare, work)
}

/// 按 orphan 取向取 tag 目标 commit sha (orphan = 同树无 parent 快照)。
fn snapshot_target(work: &Path, orphan: bool) -> String {
    if orphan {
        sh(work, &["commit-tree", "HEAD^{tree}", "-m", "snapshot"])
            .trim()
            .to_string()
    } else {
        fs::write(work.join("second.txt"), "history").unwrap();
        sh(work, &["add", "-A"]);
        sh(work, &["commit", "-qm", "second"]);
        sh(work, &["rev-parse", "HEAD"]).trim().to_string()
    }
}

/// 在 work 打 tag (可选 annotated) 并推至 bare。
fn push_tag(work: &Path, bare: &Path, tag: &str, target: &str, annotated: bool) {
    if annotated {
        sh(work, &["tag", "-a", "-m", "v", tag, target]);
    } else {
        sh(work, &["tag", tag, target]);
    }
    sh(
        work,
        &[
            "push",
            "-q",
            bare.to_str().unwrap(),
            &format!("refs/tags/{tag}"),
        ],
    );
}

fn guard(ctx: &Path, bare: &Path) -> Vec<log_skimmer::tag_guard::TagAudit> {
    guard_at(ctx, bare.to_str().unwrap(), &excludes()).unwrap_or_else(|e| panic!("采集失败: {e}"))
}

#[test]
fn e2e_orphan_clean_tag_passes() {
    let (ctx, bare, _) = fixture_remote("ok", &[("README.md", "hi")], true, Some("v0.1.0"), false);
    let audits = guard(&ctx, &bare);
    assert_eq!(audits.len(), 1);
    assert!(audits[0].ok(), "orphan 快照 + 干净树: {:?}", audits[0]);
}

#[test]
fn e2e_history_linked_tag_fails() {
    // 事故形态回归: tag 指向 dev 历史链上的提交 (有 parent)
    let (ctx, bare, _) = fixture_remote(
        "parent",
        &[("README.md", "hi")],
        false,
        Some("v0.2.0"),
        false,
    );
    let audits = guard(&ctx, &bare);
    assert_eq!(audits.len(), 1);
    assert!(!audits[0].ok() && audits[0].parents == 1, "{:?}", audits[0]);
}

#[test]
fn e2e_annotated_tags_cover_peel_line_path() {
    // annotated tag (发布面 Release UI 默认形态, 事故 v0.1.0 即此): peel 行链路
    // orphan 合规 / parented 违规, 与轻量 tag 判定一致
    let (ctx, bare, _) =
        fixture_remote("ann-ok", &[("README.md", "hi")], true, Some("v0.4.0"), true);
    assert!(guard(&ctx, &bare)[0].ok(), "annotated orphan 快照");

    let (ctx, bare, _) = fixture_remote(
        "ann-bad",
        &[("README.md", "hi")],
        false,
        Some("v0.5.0"),
        true,
    );
    let audits = guard(&ctx, &bare);
    assert!(
        !audits[0].ok() && audits[0].parents == 1,
        "annotated 历史链: {:?}",
        audits[0]
    );
}

#[test]
fn e2e_mixed_tag_face_reports_each_tag() {
    // 混合面: 合规 + 违规并存 — 聚合计数与逐 tag 报告 (排序序) 都须完整
    let (ctx, bare, work) =
        fixture_remote("mixed", &[("README.md", "hi")], true, Some("v0.1.0"), false);
    push_tag(
        &work,
        &bare,
        "v0.2.0",
        &snapshot_target(&work, false),
        false,
    );
    let audits = guard(&ctx, &bare);
    assert_eq!(audits.len(), 2, "{audits:?}");
    assert_eq!(audits[0].tag, "v0.1.0");
    assert!(audits[0].ok());
    assert_eq!(audits[1].tag, "v0.2.0");
    assert!(!audits[1].ok());
}

#[test]
fn e2e_blob_tag_is_malformed_violation_and_guard_continues() {
    // tag 指向 blob (畸形 tag 面): 是违规不是采集失败; 且不中断其余 tag 审计
    let (ctx, bare, work) =
        fixture_remote("blob", &[("README.md", "hi")], true, Some("v0.1.0"), false);
    fs::write(work.join("blob.bin"), "raw blob").unwrap();
    let blob = sh(&work, &["hash-object", "-w", "blob.bin"])
        .trim()
        .to_string();
    push_tag(&work, &bare, "blob-tag", &blob, false);
    let audits = guard(&ctx, &bare);
    assert_eq!(audits.len(), 2, "畸形 tag 不中断其余审计: {audits:?}");
    let bt = audits.iter().find(|a| a.tag == "blob-tag").unwrap();
    assert_eq!(bt.malformed.as_deref(), Some("blob"), "{bt:?}");
    assert!(!bt.ok());
    assert!(audits.iter().find(|a| a.tag == "v0.1.0").unwrap().ok());
}

#[test]
fn e2e_orphan_with_excluded_file_fails() {
    let (ctx, bare, _) = fixture_remote(
        "dirty",
        &[("README.md", "hi"), ("AGENTS.md", "内网维护叙述")],
        true,
        Some("v0.3.0"),
        false,
    );
    let audits = guard(&ctx, &bare);
    assert!(!audits[0].ok(), "{:?}", audits[0]);
    assert_eq!(audits[0].hits, ["AGENTS.md"]);
}

#[test]
fn e2e_empty_tag_face_passes() {
    let (ctx, bare, _) = fixture_remote("empty", &[("README.md", "hi")], true, None, false);
    assert!(guard(&ctx, &bare).is_empty(), "零 tag = 发布面当前默认形态");
}

#[test]
fn e2e_unreachable_remote_is_collect_error() {
    let d = scratch("dead");
    let ctx = d.join("ctx");
    fs::create_dir_all(&ctx).unwrap();
    assert!(
        guard_at(&ctx, "/nonexistent/log-skimmer-dead.git", &excludes()).is_err(),
        "远端不可达 = 采集层失败 (exit 2), 不是判定通过"
    );
}

// ---------- 生产清单装载回归 ----------

#[test]
fn load_excludes_rejects_internal_whitespace() {
    // 路径条目专属校验 (publish-stage 的 rm 管道按行消费, 空白路径不在支持面)
    let d = scratch("excl-ws");
    let f = d.join("ex.txt");
    fs::write(&f, "AGENTS.md\nfoo bar\n").unwrap();
    assert!(load_excludes(&f).is_err(), "内部空白条目 = 笔误, 拒绝");
}

#[test]
fn production_excludes_file_loads() {
    // publish-stage rm 与 tag-guard 校验共源; 清单漂移 (语法/空/重复) 在此暴露。
    // 发布面形态 skip (engine/tests 随发布面发布, data/ 整目录是排除件):
    // 以 AGENTS.md (同为内网专用件) 在场为 dev 面标记 — 它在而清单不在 = dev 面
    // 数据丢失, 仍然硬红; 两者皆缺 = 发布面, 跳过 (axioms.rs real_rules_load 同款)。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let f = root.join("data/publish-excludes.txt");
    if !f.exists() && !root.join("AGENTS.md").exists() {
        eprintln!("skip: 发布面形态 (无 data/publish-excludes.txt)");
        return;
    }
    let ex = load_excludes(&f).unwrap_or_else(|e| panic!("生产排除清单装载失败: {e}"));
    assert!(
        ex.contains(&"AGENTS.md".to_string()),
        "关键条目在场: {ex:?}"
    );
    assert!(
        ex.contains(&"data/".to_string()),
        "目录条目尾斜杠形态: {ex:?}"
    );
}

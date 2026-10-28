//! 凭据形态卫生检查 (机械防线) — 同树镜像模型 (2026-10-20 转向) 下本仓树即公开树,
//! 凭据误提交零容忍 (关键隐私红线 — 唯一机械防线)。
//!
//! 职责边界: 装载凭据形态清单 (外置数据文件 data/secret-patterns.txt, 一行一正则,
//!   regex crate 语法; 清单全为通用凭据形态, 零内网情报, 可公开) → 递归扫描待发布
//!   内容树 → 命中即 fail (CLI 退出码 1), 零命中放行。
//!   演进: 此前为字面子串的内网端点模式扫描 (issue #18 快照式时代的 publish 门禁);
//!   同树转向后内网情报治理改源头脱敏, 本防线转性为凭据形态门禁并升级正则语义
//!   (形态约束需要字符类/量词, 子串表达不了)。
//!   实现约束: 二进制文件 (NUL 探测) 跳过; 符号链接不跟随 (扫描域 = 目标树自身);
//!   路径字典序遍历, 报告稳定可 diff。

use crate::messages;
use std::path::{Path, PathBuf};

use regex::Regex;

/// 单条命中记录 (file:line 级定位, 门禁报告直接消费)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HygieneHit {
    /// 相对所扫目录根的路径
    pub path: PathBuf,
    /// 命中行号 (1-based)
    pub line: usize,
    /// 命中的模式原文
    pub pattern: String,
    /// 命中行摘要 (去首尾空白, 上限 120 字符)
    pub text: String,
}

/// 模式清单装载: 行式装载 + 逐条正则编译 (原文保留作命中报告)。
/// 编译失败即 Err — 清单笔误 (非法正则) 在装载期暴露, 不静默失明。
/// 文案单点: engine::messages (issue #76)。
pub fn load_patterns(path: &Path) -> Result<Vec<(String, Regex)>, String> {
    load_lines(path)?
        .into_iter()
        .map(|p| {
            let re = Regex::new(&p).map_err(|e| messages::hygiene_bad_regex(path, &p, &e))?;
            Ok((p, re))
        })
        .collect()
}

/// 行式清单装载器 (通用)。
/// 语义: 一行一条目; 空行与 `#` 独占的注释行忽略; 两端 trim; 返回排序序
/// (命中报告稳定可 diff)。
/// 校验: 文件可读 / 至少一条有效条目 (空清单 = 防线失效, 拒绝) / 无重复
/// (清单手维护, 重复是笔误信号) / 条目行不含 `#` (行内注释是 gitignore 事故
/// 同款笔误形态 — 整行成为含 `#` 的字面模式, 防线静默失效)。
pub(crate) fn load_lines(path: &Path) -> Result<Vec<String>, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| messages::hygiene_list_read_failed(path, &e))?;
    let mut pats: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    if let Some(bad) = pats.iter().find(|p| p.contains('#')) {
        return Err(messages::hygiene_inline_comment(path, bad));
    }
    pats.sort_unstable();
    if let Some(i) = (1..pats.len()).find(|&i| pats[i - 1] == pats[i]) {
        return Err(messages::hygiene_duplicate(path, &pats[i]));
    }
    if pats.is_empty() {
        return Err(messages::hygiene_empty_list(path));
    }
    Ok(pats)
}

/// 扫描目录 (递归); 返回全部命中 (路径字典序), 空 = 树干净。
pub fn scan(dir: &Path, patterns: &[(String, Regex)]) -> Vec<HygieneHit> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let mut hits = Vec::new();
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.contains(&0) {
            continue; // 二进制 (NUL 探测): 跳过
        }
        let rel = path.strip_prefix(dir).unwrap_or(&path);
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            hits.extend(
                patterns
                    .iter()
                    .filter(|(_, re)| re.is_match(line))
                    .map(|(raw, _)| HygieneHit {
                        path: rel.to_path_buf(),
                        line: i + 1,
                        pattern: raw.clone(),
                        text: line.trim().chars().take(120).collect(),
                    }),
            );
        }
    }
    hits
}

/// 递归收集常规文件 (file_type 判定不跟随符号链接; 条目读取失败静默跳过)。
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            walk(&p, out);
        } else if ft.is_file() {
            out.push(p);
        }
    }
}

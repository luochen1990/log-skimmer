//! 发布面卫生检查 (机械防线) — issue #18 (发布面暴露内网端点) 后的 publish 门禁。
//!
//! 职责边界: 装载内网模式清单 (外置数据文件, 一行一字面 substring 模式) → 递归
//!   扫描待发布内容树 → 命中即 publish fail (CLI 退出码 1), 零命中放行。
//!   清单与逻辑分离 (同构于规则集 folksonomy): 清单本身是内网端点情报, 为内网
//!   专用件 (justfile publish-stage 排除, 不入发布面) — 本文件只有装载与扫描
//!   逻辑, 零内网字面量, 可全树自扫无豁免洞。
//!   实现约束: 模式均为字面量匹配, 无 regex 必要; 二进制文件 (NUL 探测) 跳过;
//!   符号链接不跟随 (扫描域 = 发布树自身); 路径字典序遍历, 报告稳定可 diff。

use std::path::{Path, PathBuf};

/// 单条命中记录 (file:line 级定位, publish fail 报告直接消费)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HygieneHit {
    /// 相对所扫目录根的路径
    pub path: PathBuf,
    /// 命中行号 (1-based)
    pub line: usize,
    /// 命中的内网模式
    pub pattern: String,
    /// 命中行摘要 (去首尾空白, 上限 120 字符)
    pub text: String,
}

/// 装载模式清单 (一行一模式; 空行与 `#` 独占的注释行忽略; 返回为排序序 —
/// 命中报告稳定可 diff)。
/// 校验: 文件可读 / 至少一条有效模式 (空清单 = 防线失效, 拒绝) / 无重复
/// (清单手维护, 重复是笔误信号) / 模式行不含 `#` (行内注释是 gitignore 事故
/// 同款笔误形态 — 整行成为永不匹配的字面模式, 防线静默失效)。
pub fn load_patterns(path: &Path) -> Result<Vec<String>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "{}: {e} (清单为内网专用件, 须随 dev 仓在场)",
            path.display()
        )
    })?;
    let mut pats: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    if let Some(bad) = pats.iter().find(|p| p.contains('#')) {
        return Err(format!(
            "{}: 模式含 # (行内注释?) — {bad:?}; 注释须独占一行",
            path.display()
        ));
    }
    pats.sort_unstable();
    if let Some(i) = (1..pats.len()).find(|&i| pats[i - 1] == pats[i]) {
        return Err(format!("{}: 模式重复 ({})", path.display(), pats[i]));
    }
    if pats.is_empty() {
        return Err(format!("{}: 零有效模式 — 空清单即防线失效", path.display()));
    }
    Ok(pats)
}

/// 扫描目录 (递归); 返回全部命中 (路径字典序), 空 = 树干净。
pub fn scan(dir: &Path, patterns: &[String]) -> Vec<HygieneHit> {
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
                    .filter(|pat| line.contains(pat.as_str()))
                    .map(|pat| HygieneHit {
                        path: rel.to_path_buf(),
                        line: i + 1,
                        pattern: pat.clone(),
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

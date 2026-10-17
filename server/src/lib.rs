//! Log Skimmer MCP server 纯逻辑层 (协议无关, 可直接单测)。
//!
//! 职责边界: 规则集装载定位与 fail-fast 判定 / A0 行归一 / skim 结果的
//! 消费方视图聚合 (块摘要 + 代表 fragment + 双粒度统计) / read_around 原文窗口 /
//! 规则清单视图。
//!
//! 语义 SSOT: docs/rule-format.md (fragment 输出协议与消费方判定矩阵见其 §3)。
//! 状态机与提取逻辑全部复用 engine lib (本层禁止复制实现, 只做包装聚合)。

use log_skimmer::{
    load_rules, override_context, skim, Category, Fragment, Intent, Kind, Rule, Scope,
};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub mod mcp;

/// 单次调用的日志大小硬上限 (字节)。
/// rmcp stdio transport 为逐行 JSON, 框架层无帧上限; 2026-10-16 实测 1.36MB 语料
/// 全链路无截断 (tests/stdio_smoke.rs 的大样本门控用例)。此上限是防御性边界
/// (防超大粘贴拖垮内存), 触发时明确报错并指导调用方预处理截取。
pub const MAX_LOG_BYTES: usize = 32 * 1024 * 1024;

/// read_around 单次窗口行数上限 (防上下文爆炸; 需要更多请分段调用)。
pub const MAX_WINDOW_LINES: usize = 200;

/// 零块警示语义 (docs/rule-format.md §3 判定矩阵): 零块 = 规则集对该日志
/// 结构失明, 与 job 成败无关, 消费方无条件 fallback — 消费方只需检查
/// `fallback` 字段存在即一行判定。
pub const FALLBACK_NO_BLOCKS: &str = "no-blocks";
pub const ADVICE_NO_BLOCKS: &str = "零块=失明, 请回退 rg 基线 (如 rg -i 'error|fail|panic' <log>) 并考虑贡献新规则: https://github.com/luochen1990/log-skimmer-rules";

// ---------------------------------------------------------------------------
// 规则集装载
// ---------------------------------------------------------------------------

/// 规则目录定位, 优先级: env `LOG_SKIMMER_RULES` > 可执行文件相邻探测 > 编译期源码树。
/// pub: main 装载入口需要把「最终定位到的目录」带进启动日志与报错 (装载语义 SSOT 在此)。
///
/// - env: 显式指定, 部署方完全可控, 恒最优先;
/// - exe 相邻: 覆盖两类布局 — 部署形态 (rules 与 bin 同级或 bin 的父目录同级) 与
///   cargo 布局 (bin 在 target/{debug,release}/, 仓库根 rules 在 `../../rules`);
/// - 编译期 `CARGO_MANIFEST_DIR/../engine/tests/fixtures/rules`: 开发/单测形态兜底
///   (fixture 规则集, 离线可跑; 真实规则集独立成仓经 fetch-rules 获取, 四仓拓扑);
///   部署后该路径通常不存在, 故排在 exe 相邻之后。
pub fn resolve_rules_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("LOG_SKIMMER_RULES") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in ["rules", "../rules", "../../rules"] {
                let cand = dir.join(rel);
                if cand.is_dir() {
                    return cand;
                }
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../engine/tests/fixtures/rules")
}

/// 装载规则集 (fail-fast 判定点): 目录不存在 / 加载校验失败 / 零条规则均为 Err。
/// 零条视为错误: engine `load_rules` 对空目录静默返回 Ok(vec![]), 而「至少一条规则」
/// 是 server 可用性底线 (双侧安全网依赖规则集在场, spec §0)。
pub fn load_ruleset_at(dir: &Path) -> Result<Vec<Rule>, String> {
    if !dir.is_dir() {
        return Err(format!("规则目录不存在: {}", dir.display()));
    }
    let rules = load_rules(dir).map_err(|e| e.to_string())?;
    if rules.is_empty() {
        return Err(format!("规则目录内无 *.yaml 规则: {}", dir.display()));
    }
    Ok(rules)
}

/// 超限错误文案单点 (两 tool 的 Oversized 共用; 对外契约文案防双份分叉)。
fn oversized_msg(n: usize) -> String {
    format!("log_text {n} 字节超过上限 {MAX_LOG_BYTES} 字节; 请预处理截取 (如 tail -c) 后分段提交")
}

// ---------------------------------------------------------------------------
// skim 视图
// ---------------------------------------------------------------------------

/// skim tool 的输入校验错误 (协议层转 MCP invalid_params, 非 crash)。
#[derive(Debug, PartialEq, Eq)]
pub enum SkimInputError {
    /// log_text 为空串或纯空白 — 无可扫描内容。
    Empty,
    /// log_text 超过 [MAX_LOG_BYTES], 携带实际字节数。
    Oversized(usize),
    /// context 超出 [0,10] (spec §1 context 边界), 携带非法值。
    InvalidContext(i32),
}

impl SkimInputError {
    pub fn message(&self) -> String {
        match self {
            SkimInputError::Empty => "log_text 为空或纯空白, 无可扫描内容".into(),
            SkimInputError::Oversized(n) => oversized_msg(*n),
            SkimInputError::InvalidContext(n) => {
                format!("context 须在 0-10 内, 收到 {n}")
            }
        }
    }
}

/// 输出块摘要: 消费方快速浏览层 (intent 优先级序 — failure > warning > info, 同级按行序)。
#[derive(Debug, Serialize)]
pub struct BlockSummary {
    /// 输出块区间 hull (含 context 扩展, 1-based 含端点)。
    pub start_line: usize,
    pub end_line: usize,
    /// 块内命中的规则 id 全集 (首次出现序去重, engine 侧语义)。
    pub rules: Vec<String>,
    /// 代表 fragment 的 intent (块内信息优先级最高者)。
    pub intent: Intent,
    /// 代表 fragment 的 category。
    pub category: Category,
    /// 块内呈现行数 (noise 剔除后)。
    pub out_lines: usize,
    /// 块内签名行 (各 fragment 锚行) 去重计数。
    pub signature_lines: usize,
}

/// skim tool 输出 (消费方判定: `fallback` 字段存在即零块失明, 其余字段照常可用)。
/// blocks 与 fragments 同序一一对应, 排序契约见 [skim_view] 头注释。
#[derive(Debug, Serialize)]
pub struct SkimOutput {
    pub blocks: Vec<BlockSummary>,
    /// 与 blocks 一一对应的代表 fragment (全字段, spec §3 恒带字段集)。
    pub fragments: Vec<Fragment>,
    pub stats: StatsView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advice: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StatsView {
    pub total_lines: usize,
    pub out_lines: usize,
    pub signature_lines: usize,
    /// 1 - out_lines/total_lines (块级粒度, 四位小数)。
    pub compression_block: f64,
    /// 1 - signature_lines/total_lines (签名行级粒度, 四位小数)。
    pub compression_signature: f64,
}

fn compression_ratio(total: usize, part: usize) -> f64 {
    // total ≥ 1 由调用方保证 (空输入已在参数校验拒绝)
    ((1.0 - part as f64 / total as f64) * 10000.0).round() / 10000.0
}

/// skim 核心包装: 复用 engine 纯函数, 聚合为消费方视图。
/// 输出排序契约: blocks/fragments 按 intent 优先级 (failure > warning > info) 排列,
/// 同级保持行序 — 排序发生在本视图层, 引擎输出层行序不变 (取舍详见函数体尾注释)。
/// `context` 语义 = CLI `--context` 全局覆盖 (before=after=N, 引擎已支持)。
pub fn skim_view(
    rules: &[Rule],
    log_text: &str,
    context: Option<i32>,
) -> Result<SkimOutput, SkimInputError> {
    if log_text.trim().is_empty() {
        return Err(SkimInputError::Empty);
    }
    if log_text.len() > MAX_LOG_BYTES {
        return Err(SkimInputError::Oversized(log_text.len()));
    }
    if let Some(n) = context.filter(|n| !(0..=10).contains(n)) {
        return Err(SkimInputError::InvalidContext(n));
    }
    let lines = log_skimmer::normalize_lines(log_text);
    let result = match context {
        None => skim("mcp", rules, &lines),
        Some(n) => skim("mcp", &override_context(rules, n as u32), &lines),
    };

    // 块摘要与代表 fragment 成对组装, 末尾按 intent 优先级统一排序 (见下)。
    let mut paired: Vec<(BlockSummary, Fragment)> = Vec::with_capacity(result.blocks.len());
    for b in &result.blocks {
        // 代表 fragment: engine 单点语义 (intent > confidence > 块内原序, 稳定取首个最优)
        let rep = b.representative();
        let out_lines = (b.start_line..=b.end_line)
            .filter(|l| !result.noise_lines.contains(l))
            .count();
        let signature_lines = b
            .fragments
            .iter()
            .map(|f| f.block.0)
            .collect::<HashSet<_>>()
            .len();
        paired.push((
            BlockSummary {
                start_line: b.start_line,
                end_line: b.end_line,
                rules: b.rules.clone(),
                intent: rep.intent,
                category: rep.category,
                out_lines,
                signature_lines,
            },
            rep.clone(),
        ));
    }

    // issue #8: blocks/fragments 按 intent 优先级排序 (failure > warning > info),
    // 同级保持行序 (start_line 升序, 稳定) — 消费方最关心失败证据, 失败块须排 info 块前。
    // 取舍记录: spec §2 A7「输出块按行序排列」约束的是引擎输出层 (SkimResult.blocks,
    // 即 CLI 分节渲染与 CLI --json 的直接数据源), 该层保持行序不动 (a7 单测契约不变);
    // 本层是 MCP JSON 元数据数组的组装点 (skim tool 消费面, issue #8 的观察面), 重排仅
    // 发生在此。A7 第二子句「引擎不排序 fragment 内容」不受影响 — 排序只动数组元素序,
    // 不动块内文本行序; L0 聚合是顺序无关计数, 压缩率数字不受影响。
    paired.sort_by_key(|(b, _)| (log_skimmer::intent_rank(b.intent), b.start_line));
    let (blocks, fragments): (Vec<BlockSummary>, Vec<Fragment>) = paired.into_iter().unzip();

    let total = result.stats.input_lines;
    let stats = StatsView {
        total_lines: total,
        out_lines: result.stats.output_lines,
        signature_lines: result.stats.signature_output_lines,
        compression_block: compression_ratio(total, result.stats.output_lines),
        compression_signature: compression_ratio(total, result.stats.signature_output_lines),
    };
    let (fallback, advice) = if blocks.is_empty() {
        (
            Some(FALLBACK_NO_BLOCKS.to_string()),
            Some(ADVICE_NO_BLOCKS.to_string()),
        )
    } else {
        (None, None)
    };
    Ok(SkimOutput {
        blocks,
        fragments,
        stats,
        fallback,
        advice,
    })
}

// ---------------------------------------------------------------------------
// read_around 视图
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum ReadAroundError {
    EmptyLog,
    Oversized(usize),
    /// line_start < 1 或 line_start > line_end, 携带原始入参。
    BadRange(i64, i64),
    /// line_start 超出日志末行 (窗口为空), 携带 (总行数, line_start)。
    StartBeyondTotal(usize, i64),
}

impl ReadAroundError {
    pub fn message(&self) -> String {
        match self {
            ReadAroundError::EmptyLog => "log_text 为空或纯空白, 无可读窗口".into(),
            ReadAroundError::Oversized(n) => oversized_msg(*n),
            ReadAroundError::BadRange(s, e) => {
                format!("行号区间非法 (须 1 ≤ line_start ≤ line_end), 收到 start={s} end={e}")
            }
            ReadAroundError::StartBeyondTotal(total, s) => {
                format!("line_start={s} 超出日志末行 (共 {total} 行)")
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ReadAroundResult {
    pub total_lines: usize,
    /// 实际返回窗口 (clamp 与截断之后), 1-based 含端点。
    pub window_start: usize,
    pub window_end: usize,
    /// 是否因超过 [MAX_WINDOW_LINES] 上限被截断 (clamp 到日志末尾不算截断)。
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 带行号前缀的原文行: `L{n}: <text>`。
    pub lines: Vec<String>,
}

/// 原文窗口读取 (agent 拿 skim 块坐标来取上下文原文):
/// line_end 超出日志末尾自动收缩 (拿 block 尾部 + 余量场景常见, 宽容处理);
/// 窗口超 [MAX_WINDOW_LINES] 截断并在 note 给出续读起点。
pub fn read_around_view(
    log_text: &str,
    line_start: i64,
    line_end: i64,
) -> Result<ReadAroundResult, ReadAroundError> {
    if log_text.trim().is_empty() {
        return Err(ReadAroundError::EmptyLog);
    }
    if log_text.len() > MAX_LOG_BYTES {
        return Err(ReadAroundError::Oversized(log_text.len()));
    }
    if line_start < 1 || line_end < line_start {
        return Err(ReadAroundError::BadRange(line_start, line_end));
    }
    let lines = log_skimmer::normalize_lines(log_text);
    let total = lines.len();
    let start = line_start as usize;
    if start > total {
        return Err(ReadAroundError::StartBeyondTotal(total, line_start));
    }
    let requested_end = (line_end as usize).min(total);
    let window_end = requested_end.min(start + MAX_WINDOW_LINES - 1);
    let truncated = window_end < requested_end;
    let note = truncated.then(|| {
        format!(
            "请求窗口 L{start}-L{line_end} 超过 {MAX_WINDOW_LINES} 行上限, 已截断; 续读请以 L{} 为 line_start 再次调用",
            window_end + 1
        )
    });
    let lines = (start..=window_end)
        .map(|n| format!("L{n}: {}", lines[n - 1]))
        .collect();
    Ok(ReadAroundResult {
        total_lines: total,
        window_start: start,
        window_end,
        truncated,
        note,
        lines,
    })
}

// ---------------------------------------------------------------------------
// list_rules 视图
// ---------------------------------------------------------------------------

/// 规则清单条目 (agent 判断规则集覆盖面用)。
/// kind 为补充字段: noise 规则无 intent (恒 null), 无 kind 则该行不可自解释。
#[derive(Debug, Serialize)]
pub struct RuleView {
    pub id: String,
    pub tool: String,
    pub kind: Kind,
    /// 有效 intent (signal 语义默认 failure); noise 规则无 intent (null, spec §1)。
    pub intent: Option<Intent>,
    pub category: Category,
    pub scope: Scope,
    pub repo: Option<String>,
}

/// 装载规则清单, 按 id 排序 (输出可预测, 与装载顺序解耦)。
pub fn list_rules_view(rules: &[Rule]) -> Vec<RuleView> {
    let mut views: Vec<RuleView> = rules
        .iter()
        .map(|r| RuleView {
            id: r.id.clone(),
            tool: r.tool.clone(),
            kind: r.kind,
            intent: (r.kind != Kind::Noise).then(|| Fragment::effective_intent(r)),
            category: r.category,
            scope: r.scope,
            repo: r.repo.clone(),
        })
        .collect();
    views.sort_by(|a, b| a.id.cmp(&b.id));
    views
}

// ---------------------------------------------------------------------------
// 单测 (纯函数层; 协议包装与 stdio 链路测试见 mcp.rs 与 tests/)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<Rule> {
        load_ruleset_at(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../engine/tests/fixtures/rules")
                .as_path(),
        )
        .expect("fixture 规则集装载成功")
    }

    #[test]
    fn skim_input_validation() {
        let rs = rules();
        assert_eq!(skim_view(&rs, "", None).unwrap_err(), SkimInputError::Empty);
        assert_eq!(
            skim_view(&rs, "  \n\t \n", None).unwrap_err(),
            SkimInputError::Empty
        );
        assert_eq!(
            skim_view(&rs, "x", Some(11)).unwrap_err(),
            SkimInputError::InvalidContext(11)
        );
        assert_eq!(
            skim_view(&rs, "x", Some(-1)).unwrap_err(),
            SkimInputError::InvalidContext(-1)
        );
        // #17: 错误消息自明, 不含内部 spec 章节指针
        let err = skim_view(&rs, "x", Some(11)).unwrap_err();
        assert_eq!(err, SkimInputError::InvalidContext(11));
        let msg = err.message();
        assert!(msg.contains("context 须在 0-10 内, 收到 11"), "{msg}");
        assert!(!msg.contains("spec §"), "{msg}");
        let big = "x".repeat(MAX_LOG_BYTES + 1);
        assert_eq!(
            skim_view(&rs, &big, None).unwrap_err(),
            SkimInputError::Oversized(big.len())
        );
    }

    #[test]
    fn skim_zero_blocks_fallback_shape() {
        let rs = rules();
        let out = skim_view(&rs, "nothing matchable here\nplain text\n", None).unwrap();
        assert!(out.blocks.is_empty());
        assert!(out.fragments.is_empty());
        assert_eq!(out.fallback.as_deref(), Some(FALLBACK_NO_BLOCKS));
        assert_eq!(out.advice.as_deref(), Some(ADVICE_NO_BLOCKS));
        assert_eq!(out.stats.total_lines, 2);
        assert_eq!(out.stats.out_lines, 0);
    }

    /// issue #8: blocks/fragments 按 intent 优先级排序 (failure > warning > info),
    /// 同级保持行序 — info 块在前 failure 块在后时, blocks[0] 须为 failure。
    #[test]
    fn skim_blocks_intent_priority_order() {
        let rs = rules();
        // info 块 (L1) 与 failure 块 (L3) 隔行不合并, 引擎行序 info 在前
        let log = "Runner Image: ubuntu-24.04\nplain line\nerror: boom\n";
        let out = skim_view(&rs, log, None).unwrap();
        assert_eq!(out.blocks.len(), 2);
        assert_eq!(out.blocks[0].intent, Intent::Failure, "failure 块排首");
        assert_eq!(out.blocks[0].start_line, 3);
        assert_eq!(out.blocks[1].intent, Intent::Info);
        assert_eq!(out.blocks[1].start_line, 1);
        assert_eq!(
            out.fragments[0].rule, "nix-eval-error-tree",
            "fragments 与 blocks 同序 (一一对应)"
        );
        // 同级保持行序: 两个 failure 块按 start_line 升序
        let log2 = "error: first\nx\nerror: second\n";
        let out2 = skim_view(&rs, log2, None).unwrap();
        let starts: Vec<usize> = out2.blocks.iter().map(|b| b.start_line).collect();
        assert_eq!(starts, vec![1, 3], "同 intent 块行序稳定");
    }

    #[test]
    fn skim_hit_shapes_blocks_and_fragments() {
        let rs = rules();
        let log =
            "cargo build\nerror: could not compile `demo` (bin \"demo\") due to 1 previous error\n";
        let out = skim_view(&rs, log, None).unwrap();
        assert!(!out.blocks.is_empty());
        assert_eq!(out.blocks.len(), out.fragments.len());
        let b = &out.blocks[0];
        assert!(b.rules.contains(&"cargo-could-not-compile".to_string()));
        assert_eq!(b.intent, Intent::Failure);
        assert_eq!(b.category, Category::Build);
        assert!(b.out_lines >= 1);
        assert!(b.signature_lines >= 1);
        let f = &out.fragments[0];
        assert_eq!(f.rule, "cargo-could-not-compile");
        assert!(f.text.contains("could not compile"));
        assert_eq!(out.stats.total_lines, 2);
        // 块级 out_lines 求和可大于全局 out_lines: A5 合并按 block 区间判定, 而块级
        // 统计按输出区间 hull — 相邻块经 context 扩展后输出区间可重叠, 全局侧去重。
        assert!(out.stats.out_lines <= out.blocks.iter().map(|b| b.out_lines).sum::<usize>());
        // 序列化形态: spec §3 恒带字段全在场
        let json = serde_json::to_value(&out).unwrap();
        let frag0 = &json["fragments"][0];
        for key in [
            "rule",
            "kind",
            "intent",
            "category",
            "confidence",
            "block",
            "out",
            "text",
        ] {
            assert!(frag0.get(key).is_some(), "fragment 缺恒带字段 {key}");
        }
    }

    #[test]
    fn read_around_window_semantics() {
        let log = (1..=10)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        // 正常窗口 + 行号前缀
        let r = read_around_view(&log, 2, 4).unwrap();
        assert_eq!(r.lines, vec!["L2: line2", "L3: line3", "L4: line4"]);
        assert_eq!(r.total_lines, 10);
        assert!(!r.truncated);
        assert!(r.note.is_none());
        // line_end 超出末尾 → clamp (非截断)
        let r = read_around_view(&log, 8, 99).unwrap();
        assert_eq!(r.window_end, 10);
        assert!(!r.truncated);
        // 越界/非法区间
        assert_eq!(
            read_around_view(&log, 0, 3).unwrap_err(),
            ReadAroundError::BadRange(0, 3)
        );
        assert_eq!(
            read_around_view(&log, 5, 2).unwrap_err(),
            ReadAroundError::BadRange(5, 2)
        );
        assert_eq!(
            read_around_view(&log, 11, 20).unwrap_err(),
            ReadAroundError::StartBeyondTotal(10, 11)
        );
        assert_eq!(
            read_around_view("", 1, 2).unwrap_err(),
            ReadAroundError::EmptyLog
        );
    }

    #[test]
    fn read_around_truncation_at_200() {
        let log = (1..=1000)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let r = read_around_view(&log, 5, 900).unwrap();
        assert!(r.truncated);
        assert_eq!(r.lines.len(), MAX_WINDOW_LINES);
        assert_eq!(r.window_start, 5);
        assert_eq!(r.window_end, 5 + MAX_WINDOW_LINES - 1);
        assert!(
            r.note.as_deref().unwrap().contains("L205"),
            "续读起点 = 截断尾行+1"
        );
        assert_eq!(r.lines.last().unwrap(), "L204: l204");
        // 恰好 200 行不截断
        let r = read_around_view(&log, 1, 200).unwrap();
        assert!(!r.truncated);
        assert_eq!(r.lines.len(), 200);
    }

    #[test]
    fn list_rules_view_sorted_and_complete() {
        let views = list_rules_view(&rules());
        assert!(
            views.len() >= 2,
            "fixture 规则集基线 (真实规则集规模由 l0 链把守)"
        );
        let ids: Vec<&str> = views.iter().map(|v| v.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "按 id 升序");
        for v in &views {
            assert!(v.id.starts_with(&format!("{}-", v.tool)));
            // noise 无 intent / signal 恒有有效 intent (spec §1)
            assert_eq!(v.intent.is_none(), v.kind == Kind::Noise);
        }
    }
}

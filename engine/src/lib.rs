//! Log Skimmer POC 引擎核心。
//!
//! 职责边界: 规则加载 (含 spec §1 全部加载期校验) + 逐行状态机扫描 (spec §2 公理 A1-A8)
//! + fragment 输出聚合 + L0 统计。核心 `skim` 是纯函数 (无 IO/时间/随机), CLI 是薄壳。
//!
//! 语义 SSOT: docs/rule-format.md — 实现与其冲突时以 spec 为准。

pub mod hygiene;
pub mod l0;
pub mod manifest;
pub mod sample_io;

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;

// ---------------------------------------------------------------------------
// 类型 (spec §1 规则格式 / §3 输出协议)
// ---------------------------------------------------------------------------

/// 单个 pattern: 正则原文 + 编译产物 + loop 标志。
/// deny_unknown_fields (spec §1.M): 未知字段拒载, 防 pattern 级字段静默偏移。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub regex: String,
    /// 贪心重复消费标志 (YAML 键 `loop`, Rust 关键字故重命名)。
    #[serde(rename = "loop", default)]
    pub is_loop: bool,
    #[serde(skip)]
    pub compiled: Option<Regex>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Signal,
    Noise,
}

/// 命中语义 (spec §1): failure = 失败证据 (成功 job 命中即 FP);
/// warning = 有问题倾向但非失败 (deprecation 类); info = 纯环境快照 (版本行等)。
/// warning/info 均不计 FP、不参与 FN。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    Failure,
    Warning,
    Info,
}

// Display 与 serde 序列化名恒一致 (display_names_match_serde 单测锁死) —
// CLI 人类可读块头与 JSON 协议共用同一套名字。
impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Intent::Failure => "failure",
            Intent::Warning => "warning",
            Intent::Info => "info",
        })
    }
}

/// intent 信息优先级: failure (失败证据) > warning (问题倾向) > info (环境快照)。
/// 块头代表 fragment 选择 (CLI 人类可读) 与 MCP 视图排序共用的单点定义。
pub fn intent_rank(i: Intent) -> u8 {
    match i {
        Intent::Failure => 0,
        Intent::Warning => 1,
        Intent::Info => 2,
    }
}

fn confidence_rank(c: Confidence) -> u8 {
    match c {
        Confidence::High => 0,
        Confidence::Medium => 1,
        Confidence::Low => 2,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// category 封闭枚举 (spec §1, 调研定稿 10 值; 省缺注入 Unknown — 宁缺勿错)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Fmt,
    Lint,
    Typecheck,
    Build,
    Test,
    Security,
    Deps,
    Infra,
    Config,
    #[default]
    Unknown, // 省缺注入 (宁缺勿错)
}

// Display 与 serde 序列化名恒一致 (display_names_match_serde 单测锁死)。
impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Category::Fmt => "fmt",
            Category::Lint => "lint",
            Category::Typecheck => "typecheck",
            Category::Build => "build",
            Category::Test => "test",
            Category::Security => "security",
            Category::Deps => "deps",
            Category::Infra => "infra",
            Category::Config => "config",
            Category::Unknown => "unknown",
        })
    }
}

/// context 取值上界 (含): spec §1 字段约束表 `context.before/after: 0 ≤ 值 ≤ 10`
/// 是取值 SSOT, 本常量是其代码侧单源 (issue #28)。消费面三处同界引用:
/// 规则加载校验 (下方 validate_rule) / CLI `--context` (main.rs clap range) /
/// MCP skim `context` (server 运行期校验 + inputSchema) — 改本常量即三面同步,
/// 锁界单测的边界值均从本常量派生。类型 u32 = Context 字段原生类型。
pub const CONTEXT_MAX: u32 = 10;

/// context 输出边界扩展 (spec §1)。deny_unknown_fields: 同 Rule 契约。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    #[serde(default)]
    pub before: u32,
    #[serde(default)]
    pub after: u32,
}

/// 一条规则 (spec §1)。加载期校验后 compiled 就绪。
/// deny_unknown_fields (spec §1.M, issue #59): 未知字段从「静默忽略」变「显式拒载」
/// — 静默偏移 (规则语义已变而提取结果无感知漂移) 比拒载更危险; 新字段随引擎支持引入。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub title: String,
    pub tool: String,
    /// 弃用窗口 (format_version 2, spec §1.S): scope/repo 字段退役 — 生效范围由
    /// 存储位置唯一决定 (中央仓 = 全局层 / 项目仓 `.log-skimmer/rules/` = 项目层)。
    /// 在场容忍但值被完全忽略: 不校验任何约束 (含 v1 视为非法的组合形态也放行,
    /// 保证 fetch-rules 拉到的旧规则集零破坏装载); 禁止新规则使用; v3 物理删除。
    /// 类型取 serde_yaml::Value 而非旧枚举: 既然忽略值, 就不对值域做任何假设。
    #[serde(default)]
    pub scope: Option<serde_yaml::Value>,
    /// 同上 — scope 的伴生字段, 一同进入弃用窗口。
    #[serde(default)]
    pub repo: Option<serde_yaml::Value>,
    pub patterns: Vec<Pattern>,
    pub kind: Kind,
    /// None = YAML 缺省 (signal 语义默认 Failure; noise 必须缺省, 见校验)。
    #[serde(default)]
    pub intent: Option<Intent>,
    #[serde(default)]
    pub category: Category,
    #[serde(default = "default_confidence")]
    pub confidence: Confidence,
    #[serde(default)]
    pub context: Context,
    // structured-hint / attribution: spec 保留字段, POC 不消费, 但须容忍存在
    // (YAML 键为连字符形态 structured-hint — deny_unknown_fields 下须 rename 对齐)。
    #[serde(default, rename = "structured-hint")]
    pub structured_hint: Option<serde_yaml::Value>,
    #[serde(default)]
    pub attribution: Option<serde_yaml::Value>,
}

fn default_confidence() -> Confidence {
    Confidence::Medium
}

/// fragment: 一条规则一次闭块的产物 (spec §3)。
#[derive(Debug, Clone, Serialize)]
pub struct Fragment {
    pub rule: String,
    pub kind: Kind,
    /// 有效 intent (signal 恒有值: 显式或默认 Failure)。
    pub intent: Intent,
    pub category: Category,
    pub confidence: Confidence,
    /// 该规则自身消费区间 (无 context), 1-based 含端点。
    pub block: (usize, usize),
    /// 输出区间 (block ∪ context 扩展, clamp 后), 1-based 含端点。
    pub out: (usize, usize),
    /// 输出区间内文本 (noise 行已剔除; 全被剔则该 fragment 被 skim 丢弃, 不会出现空 text)。
    pub text: String,
}

impl Fragment {
    /// 规则有效 intent: signal 语义默认 Failure。
    pub fn effective_intent(rule: &Rule) -> Intent {
        rule.intent.unwrap_or(Intent::Failure)
    }
}

/// 输出块: 相邻/重叠 signal fragment 的聚合 (A5)。
#[derive(Debug, Clone, Serialize)]
pub struct OutputBlock {
    pub start_line: usize,
    pub end_line: usize,
    /// 块内命中规则 id 全集, 首次出现序去重 (同规则多锚行合并进同块只记一次)。
    pub rules: Vec<String>,
    pub fragments: Vec<Fragment>,
    /// 折叠标记 (spec §3.R2): 仅折叠 run 的锚块在场 — 同规则级联块的呈现层
    /// 折叠索引。None = 该块未折叠任何后续块。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collapse: Option<CollapseMarker>,
}

/// 输出层折叠参数 (spec §3.R4): 引擎级, 默认 (10, 3); 阈值不满足时输出与
/// 未折叠引擎逐字节一致 (零行为变更保证)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollapseParams {
    /// 同规则相邻块的块区间最大间隔 (行, context 不参与)。
    /// 默认 20: 语料块区间 gap 双簇 — 密集簇 ≤8 + 重复单元簇 13-15 (swiftc
    /// 级联诊断单元 16 行) — 20 盖住双簇 + 余量 (spec §3.R1)。
    pub max_gap: usize,
    /// 折叠 run 最小块数 (含锚块; 有效下限 2)。默认 3: len=2 最普遍且折半
    /// 信息不折, 只折重复模式已建立的级联 (spec §3.R1)。
    pub min_repeat: usize,
}

impl Default for CollapseParams {
    fn default() -> Self {
        CollapseParams {
            max_gap: 20,
            min_repeat: 3,
        }
    }
}

impl CollapseParams {
    /// 折叠完全关闭 — 任何 run 都不形成, 与未折叠引擎逐字节等价。
    pub const OFF: CollapseParams = CollapseParams {
        max_gap: 0,
        min_repeat: usize::MAX,
    };

    /// 折叠是否可能发生 (max_gap ≥ 1 且 min_repeat 有效下限 2)。
    pub fn is_enabled(&self) -> bool {
        self.max_gap >= 1 && self.min_repeat >= 2
    }
}

/// 折叠标记 (spec §3.R2): 锚块上的同规则级联折叠索引。字段集是统计层恢复
/// 数据面 (frag_spans + rule + intent) 与消费方展开索引 (锚行 = frag_spans
/// 首元素) 的单点 — 被 text 不保留 (展开走 read_around 原文)。
#[derive(Debug, Clone, Serialize)]
pub struct CollapseMarker {
    /// 被折叠块的单规则 id (锚块 rules 同值)。
    pub rule: String,
    /// 该规则的有效 intent (run 内全 fragment 同 rule 同 intent — 规则级常量;
    /// 统计层恢复 FN/FP 口径用)。
    pub intent: Intent,
    /// 被折叠块数 (不含锚块; 呈现单位)。
    pub repeat_blocks: usize,
    /// 被折叠块输出区间 hull (含 context, 呈现坐标参照)。
    pub start_line: usize,
    pub end_line: usize,
    /// 该 run 折叠后不再呈现的行数 (noise 剔除口径, 集合差 — spec §3.R2)。
    pub omitted_lines: usize,
    /// 被折叠各 fragment 的 block 区间升序 (证据单位; 锚行 = 各区间首元素;
    /// FP 审计可离线复算 — 区间行数可加总)。
    pub frag_spans: Vec<(usize, usize)>,
}

impl CollapseMarker {
    /// 人类可读折叠行 (spec §3.R2): 单行, 锚行最多列 8 个, 超出以
    /// `…(共 N 行)` 收尾。不计入 output_lines (元数据与块头同族不计)。
    pub fn render_line(&self) -> String {
        let mut s = format!(
            "⟨×{} {} 折叠块 [L{}-L{}] 省略 {} 行; 锚行",
            self.repeat_blocks, self.rule, self.start_line, self.end_line, self.omitted_lines
        );
        for (start, _) in self.frag_spans.iter().take(8) {
            s.push_str(&format!(" L{start}"));
        }
        if self.frag_spans.len() > 8 {
            s.push_str(&format!(" …(共 {} 行)", self.frag_spans.len()));
        }
        s.push('⟩');
        s
    }
}

/// 折叠审计计数 (spec §3.R3): `stats.collapse` 字段; 零折叠时序列化省略
/// (零行为变更保证 — 未折叠输出与 v0.6 逐字节一致)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CollapseStats {
    /// 被折叠块总数 (跨全部 run)。
    pub folded_blocks: usize,
    /// 折叠后不再呈现的行数 (全局集合差: 全部块呈现行集 − 幸存块呈现行集;
    /// 不变式 output_lines = 未折叠值 − omitted_lines)。
    pub omitted_lines: usize,
}

impl CollapseStats {
    fn is_zero(&self) -> bool {
        *self == CollapseStats::default()
    }
}

impl OutputBlock {
    /// 块代表 fragment: intent 优先级 > confidence, 同键取块内原序首个 (min_by_key 文档保证)。
    /// CLI 人类可读块头与 MCP BlockSummary 的 intent/category 取值共用此单点语义
    /// (混合 intent 合并块显示最高信息优先级一侧)。
    pub fn representative(&self) -> &Fragment {
        self.fragments
            .iter()
            .min_by_key(|f| (intent_rank(f.intent), confidence_rank(f.confidence)))
            .expect("输出块恒有 ≥1 fragment (A5 聚合不变量)")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BoundaryViolation {
    pub rule: String,
    pub line_no: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub input_lines: usize,
    pub output_lines: usize,
    /// 签名行级粒度: 各 fragment block 首行 (锚行) 去重计数 (spec §3 双粒度压缩比分列)。
    /// 折叠下保持**全量口径** (折叠块锚行照常计入, spec §3.R3)。
    pub signature_output_lines: usize,
    pub noise_lines_removed: usize,
    /// 折叠审计计数 (spec §3.R3): 零折叠时序列化省略 (零行为变更保证)。
    #[serde(skip_serializing_if = "CollapseStats::is_zero")]
    pub collapse: CollapseStats,
    /// 边界完整性机械检查违例 (spec §4): block 尾行未被该规则任一 pattern 匹配。
    /// 按状态机构造该检查恒真, 作为实现哨兵存在 — 非零 = 引擎 bug。
    pub boundary_violations: Vec<BoundaryViolation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkimResult {
    pub file: String,
    pub input_lines: usize,
    pub output_lines: usize,
    pub blocks: Vec<OutputBlock>,
    pub stats: Stats,
    /// noise 行集 (1-based) — 人类可读渲染用, 不进 JSON 协议。
    #[serde(skip)]
    pub noise_lines: HashSet<usize>,
    /// noise 规则闭块计数 (规则 id → 剔除区块数) — L0 正向验证用 (issue #19):
    /// noise 规则不产 fragment, 其「命中」只能以剔除区块计。不进 JSON 协议。
    #[serde(skip)]
    pub noise_rule_hits: BTreeMap<String, u64>,
}

// ---------------------------------------------------------------------------
// 加载与校验 (spec §1)
// ---------------------------------------------------------------------------

/// 聚合加载错误 — 一次报告全部问题 (spec §1: 整体拒绝)。
#[derive(Debug, Default)]
pub struct LoadErrors(pub Vec<String>);

impl fmt::Display for LoadErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "规则集非法 ({} 处):\n{}",
            self.0.len(),
            self.0.join("\n")
        )
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 递归读取目录下全部 `*.yaml`, 解析 + 校验 + 编译 regex。
pub fn load_rules(dir: &Path) -> Result<Vec<Rule>, LoadErrors> {
    let mut errs = LoadErrors::default();
    // 目录不存在 / 零规则 = 集体失明: 空规则集对任何日志输出零块, 在 L0 聚合侧表现
    // 为全样本 100% 压缩的假象 (实测事故: rules 仓 lint 的 base 路径基准分叉, 不存在
    // 的目录被静默当空集 → base 恒 100% → 伪 regression) — 拒绝而非静默。
    // #47a: 路径存在但非目录 (如误指单个 yaml 文件) 如实报「不是目录」,
    // 与「不存在」区分 — 误报不存在会误导排障方向。
    if !dir.is_dir() {
        if dir.exists() {
            errs.0.push(format!(
                "{}: 路径存在但不是目录 (规则集须为目录)",
                dir.display()
            ));
        } else {
            errs.0.push(format!("{}: 规则目录不存在", dir.display()));
        }
        return Err(errs);
    }
    // manifest 版本协商 (spec §1.M): 集级可选文件, 兼容性违例 = 规则集非法,
    // 整体拒载 (exit 2 族); 无 manifest 的规则集 (现存全部) 照常装载。
    manifest::check(dir, env!("CARGO_PKG_VERSION")).map_err(|e| LoadErrors(vec![e]))?;
    let mut files = Vec::new();
    collect_yaml_files(dir, &mut files);
    // manifest 保留名不作为规则解析 (spec §1.M — 根目录集级文件)。
    let manifest_path = dir.join(manifest::MANIFEST_FILENAME);
    files.retain(|p| *p != manifest_path);
    if files.is_empty() {
        errs.0.push(format!(
            "{}: 目录下无 *.yaml (规则集为空 = 引擎失明)",
            dir.display()
        ));
        return Err(errs);
    }
    files.sort();
    let mut rules = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new(); // id → 首见源文件 (重复检测)
    for path in &files {
        let rel = path.strip_prefix(dir).unwrap_or(path).display().to_string();
        let raw = match std::fs::read_to_string(path) {
            Ok(r) => r,
            Err(e) => {
                errs.0.push(format!("{rel}: 读取失败: {e}"));
                continue;
            }
        };
        load_rule_source(&rel, &raw, &mut errs, &mut rules, &mut seen);
    }
    if errs.0.is_empty() {
        Ok(rules)
    } else {
        Err(errs)
    }
}

/// 单个规则源的 解析 + 校验 + id 去重 — [load_rules] (目录路径式) 与
/// [load_rules_from_sources] (内存源式) 共用的同一条装载路径 (重构共用而非复制)。
fn load_rule_source(
    rel: &str,
    raw: &str,
    errs: &mut LoadErrors,
    rules: &mut Vec<Rule>,
    seen: &mut HashMap<String, String>,
) {
    match serde_yaml::from_str::<Rule>(raw) {
        Ok(mut rule) => {
            validate_rule(&mut rule, rel, errs);
            // id 全局唯一: 报错并列首见文件与冲突文件 (issue #14 B — 只报 id
            // 字符串在多文件规则仓里等于让人肉 grep 定位)。
            if let Some(first) = seen.get(&rule.id) {
                errs.0
                    .push(format!("id 重复: {} ({first} vs {rel})", rule.id));
            } else {
                seen.insert(rule.id.clone(), rel.to_string());
            }
            rules.push(rule);
        }
        Err(e) => errs.0.push(format!("{rel}: YAML/字段解析失败: {e}")),
    }
}

/// 规则集内存源条目: `(源名, YAML 文本)` — 源名遵循 [load_rules] 的相对路径
/// 形态 (根级 `manifest.yaml` 为集级保留名, spec §1.M)。刻意取位置元组而非
/// 具名 struct: 镜像 ABI 的 JSON 数组协议 `[[源名, YAML 文本], ...]` (serde
/// 数组形态, wasm 宿主零转换)。
pub type RuleSource = (String, String);

/// 从内存源加载规则集 — 供无文件系统环境 (wasm 宿主, `log-skimmer-wasm` ABI)
/// 传入规则集。装载语义与 [load_rules] 同构:
/// - 源名恰为 `manifest.yaml` (根级保留名, spec §1.M) → 走同一版本协商路径
///   [manifest::negotiate], 不作为规则解析; 子目录同名源 (如 `a/manifest.yaml`)
///   不在保留之列, 按规则源解析 — 与目录语义 (仅根级 manifest 免解析) 一致。
/// - 其余源逐条 解析+校验+id 去重, 共用 [load_rule_source]。
/// - 无规则源 (空列表或仅 manifest) → 拒绝: 空规则集 = 引擎失明, 与 load_rules
///   的「目录下无 *.yaml」同族拒绝。
///
/// 源序由宿主保证与目录遍历等序 (递归收集 *.yaml|*.yml 跳隐藏路径后排序);
/// 序仅影响报错罗列次序, 规则集语义与序无关。
pub fn load_rules_from_sources(sources: &[RuleSource]) -> Result<Vec<Rule>, LoadErrors> {
    let mut errs = LoadErrors::default();
    let (manifests, rule_sources): (Vec<&RuleSource>, Vec<&RuleSource>) = sources
        .iter()
        .partition(|(name, _)| name == manifest::MANIFEST_FILENAME);
    // 版本协商先于空集判定 — 与 load_rules 的检查顺序一致 (manifest 违例优先暴露)。
    for (_, raw) in &manifests {
        manifest::negotiate(raw, env!("CARGO_PKG_VERSION"), manifest::MANIFEST_FILENAME)
            .map_err(|e| LoadErrors(vec![e]))?;
    }
    if rule_sources.is_empty() {
        errs.0
            .push("(sources): 无规则源 (规则集为空 = 引擎失明)".to_string());
        return Err(errs);
    }
    let mut rules = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new(); // id → 首见源 (重复检测)
    for (rel, raw) in rule_sources {
        load_rule_source(rel, raw, &mut errs, &mut rules, &mut seen);
    }
    if errs.0.is_empty() {
        Ok(rules)
    } else {
        Err(errs)
    }
}

fn collect_yaml_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        // 隐藏路径 (如规则仓的 .github/) 是仓级元数据, 不是规则 — 跳过。
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if p.is_dir() {
            collect_yaml_files(&p, out);
        } else if p
            .extension()
            .map(|x| x == "yaml" || x == "yml")
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

fn validate_rule(rule: &mut Rule, rel: &str, errs: &mut LoadErrors) {
    if !is_valid_id(&rule.id) {
        errs.0
            .push(format!("{rel}: id 非法 (须 ^[a-z0-9-]+$): {}", rule.id));
    }
    if !rule.id.starts_with(&format!("{}-", rule.tool)) {
        errs.0.push(format!(
            "{rel}: id 须以 tool 为前缀: id={} tool={}",
            rule.id, rule.tool
        ));
    }
    // scope/repo: 弃用窗口 (format_version 2) — 字段在场容忍、值被完全忽略,
    // 不做任何校验 (v1 的 scope/repo 约束已随字段退役, spec §1.S)。
    if rule.patterns.is_empty() {
        errs.0.push(format!("{rel}: patterns 为空"));
    }
    if rule.kind == Kind::Noise && rule.intent.is_some() {
        errs.0.push(format!("{rel}: kind=noise 时 intent 必须缺省"));
    }
    if rule.context.before > CONTEXT_MAX || rule.context.after > CONTEXT_MAX {
        // 判定引用 CONTEXT_MAX 单源; 文案 "[0,10]" 是 spec 人话表述, 有意不随常量
        // 动态化 (与 server/src/lib.rs skim_view 侧同款决策)。
        errs.0.push(format!("{rel}: context 超出 [0,10]"));
    }
    for (i, p) in rule.patterns.iter_mut().enumerate() {
        match Regex::new(&p.regex) {
            Ok(c) => p.compiled = Some(c),
            Err(e) => errs
                .0
                .push(format!("{rel}: patterns[{i}] regex 编译失败: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// 状态机 (spec §2 A1-A8)
// ---------------------------------------------------------------------------

/// CLI `--context N`: 显示层全局覆盖所有规则的 context (before=N/after=N),
/// 纯机械坐标切割, 不需规则参与 (spec §3)。返回覆盖后的规则副本。
pub fn override_context(rules: &[Rule], n: u32) -> Vec<Rule> {
    rules
        .iter()
        .map(|r| {
            let mut c = r.clone();
            c.context = Context {
                before: n,
                after: n,
            };
            c
        })
        .collect()
}

/// 单规则扫描: 返回该规则全部闭块 (block 区间, 1-based 含端点)。
/// A1 独立扫描 / A2 锚开块 / A3 扩展指针贪心+透传。
fn scan_rule(rule: &Rule, lines: &[String]) -> Vec<(usize, usize)> {
    let Some(anchor) = rule.patterns[0].compiled.as_ref() else {
        return vec![];
    };
    let ext = &rule.patterns[1..];
    let mut spans = Vec::new();
    let mut i = 0usize; // 0-based 行游标
    while i < lines.len() {
        if !anchor.is_match(&lines[i]) {
            i += 1;
            continue;
        }
        // A2: 锚命中开块
        let start = i;
        let mut last = i;
        i += 1;
        // A3: 扩展指针 p (0-based 于 ext), 逐行推进
        let mut p = 0usize;
        'extend: while i < lines.len() {
            while p < ext.len() {
                if ext[p].compiled.as_ref().unwrap().is_match(&lines[i]) {
                    last = i;
                    i += 1;
                    if !ext[p].is_loop {
                        p += 1;
                    }
                    continue 'extend; // 消费了一行, 等下一行
                }
                p += 1; // 透传: 同行试下一 pattern
            }
            break; // 全部剩余 pattern 不命中 → 闭块 (当前行不属于块)
        }
        spans.push((start + 1, last + 1)); // 转 1-based
    }
    spans
}

/// A4: context 扩展 (clamp 到日志边界), 只影响输出。
fn out_span(rule: &Rule, block: (usize, usize), n: usize) -> (usize, usize) {
    (
        block.0.saturating_sub(rule.context.before as usize).max(1),
        (block.1 + rule.context.after as usize).min(n),
    )
}

/// A0 行归一 (spec §2): 首部 UTF-8 BOM 剥除 (#78, 仅文件最前一处; 行中 U+FEFF 是
/// 普通字符 ZWNBSP 不剥) + `lines()` 切行 + 剥尾部 `\r`; 尾部空行不计。
/// 引擎/CLI/server 三方唯一行预处理 (SSOT, 禁止各消费点再写同款)。
pub fn normalize_lines(raw: &str) -> Vec<String> {
    raw.strip_prefix('\u{feff}')
        .unwrap_or(raw)
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect()
}

/// 输入三分类 (spec §3, issues #71/#78): 归一后日志的内容性判定, 引擎层单点谓词。
/// CLI (Empty/WhitespaceOnly → exit 2 数据错误族) 与 MCP 入口校验 (→ invalid_params)
/// 消费同一谓词, 禁止各消费点自写空白判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputClass {
    /// 0 行 (空输入; 或仅 BOM — 剥除后为空)。
    Empty,
    /// ≥1 行且全部为空白行。
    WhitespaceOnly,
    /// 至少一行含非空白字符。
    Content,
}

/// 三分类谓词。空白口径 (规范写死): 每行所有字符均满足 `char::is_whitespace`
/// (Unicode 空白, 含全角空格 U+3000; 空行 "" 视为空白行)。
pub fn classify_input(lines: &[String]) -> InputClass {
    if lines.is_empty() {
        InputClass::Empty
    } else if lines.iter().all(|l| l.chars().all(char::is_whitespace)) {
        InputClass::WhitespaceOnly
    } else {
        InputClass::Content
    }
}

/// 输入拒斥文案 (classify_input 拒斥两态的文案 SSOT): CLI stderr 与 wasm ABI
/// envelope 共用 — 数据错误族文案 (行数 + 排障指引) 不得在消费点分叉。
/// Content → None (可扫描)。
pub fn input_reject_message(lines: &[String]) -> Option<String> {
    match classify_input(lines) {
        InputClass::Empty => Some("日志为空 (0 行): 疑似下载截断, 拒绝静默通过".to_string()),
        InputClass::WhitespaceOnly => Some(format!(
            "日志为纯空白 ({} 行均为空白行): 疑似下载截断或空跑, 请检查日志源, 拒绝静默通过",
            lines.len()
        )),
        InputClass::Content => None,
    }
}

/// A5/A6/A7 + 统计: 聚合为输出块。折叠按默认参数 (spec §3.R)。
pub fn skim(file: &str, rules: &[Rule], lines: &[String]) -> SkimResult {
    skim_with(file, rules, lines, &CollapseParams::default())
}

/// [skim] 的参数化形态 (spec §3.R4): `params` 控制输出层折叠;
/// `CollapseParams::OFF` = 与未折叠引擎逐字节等价。
pub fn skim_with(
    file: &str,
    rules: &[Rule],
    lines: &[String],
    params: &CollapseParams,
) -> SkimResult {
    let n = lines.len();
    // 第一遍: 全部规则扫描。noise 行集先行完备 (A6 与规则数组顺序无关),
    // signal 闭块区间暂存, 第二遍再构造 fragment。
    let mut noise_lines: HashSet<usize> = HashSet::new();
    let mut noise_rule_hits: BTreeMap<String, u64> = BTreeMap::new();
    let mut signal_work: Vec<(&Rule, Vec<(usize, usize)>)> = Vec::new();
    for rule in rules {
        let spans = scan_rule(rule, lines);
        if rule.kind == Kind::Noise {
            noise_rule_hits.insert(rule.id.clone(), spans.len() as u64);
            for (s, e) in &spans {
                for l in *s..=*e {
                    noise_lines.insert(l);
                }
            }
        } else {
            signal_work.push((rule, spans));
        }
    }

    let mut signals: Vec<Fragment> = Vec::new();
    let mut boundary_violations = Vec::new();
    for (rule, spans) in &signal_work {
        for block in spans {
            // 边界完整性哨兵 (spec §4): block 尾行须被该规则任一 pattern 匹配。
            if let Some(tail) = lines.get(block.1 - 1) {
                let ok = rule
                    .patterns
                    .iter()
                    .any(|p| p.compiled.as_ref().is_some_and(|c| c.is_match(tail)));
                if !ok {
                    boundary_violations.push(BoundaryViolation {
                        rule: rule.id.clone(),
                        line_no: block.1,
                        text: tail.chars().take(120).collect(),
                    });
                }
            }
            let out = out_span(rule, *block, n);
            // A6: noise 行剔除后的残行文本
            let kept: Vec<&String> = (out.0..=out.1)
                .filter(|l| !noise_lines.contains(l))
                .map(|l| &lines[l - 1])
                .collect();
            if kept.is_empty() {
                continue; // A6: signal 块全被 noise 覆盖 → 丢弃
            }
            signals.push(Fragment {
                rule: rule.id.clone(),
                kind: Kind::Signal,
                intent: Fragment::effective_intent(rule),
                category: rule.category,
                confidence: rule.confidence,
                block: *block,
                out,
                text: kept.into_iter().cloned().collect::<Vec<_>>().join("\n"),
            });
        }
    }

    // A5: 按 block 区间排序 (稳定) + 重叠/相邻合并; 输出块区间 = 成员输出区间 hull。
    signals.sort_by_key(|f| (f.block.0, f.block.1));
    let mut blocks: Vec<OutputBlock> = Vec::new();
    for f in signals {
        let mergeable = blocks.last().is_some_and(|b| {
            f.block.0 <= b.fragments.last().map(|x| x.block.1).unwrap_or(b.end_line) + 1
        });
        if mergeable {
            let b = blocks.last_mut().unwrap();
            b.end_line = b.end_line.max(f.out.1);
            b.start_line = b.start_line.min(f.out.0);
            // 同规则多锚行合并进同块只记一次 (issue #8: 逐条重复列出是展示噪音);
            // contains 线扫即首次出现序保真 — 块内规则数是个位数, 无需哈希集
            if !b.rules.contains(&f.rule) {
                b.rules.push(f.rule.clone());
            }
            b.fragments.push(f);
        } else {
            blocks.push(OutputBlock {
                start_line: f.out.0,
                end_line: f.out.1,
                rules: vec![f.rule.clone()],
                fragments: vec![f],
                collapse: None,
            });
        }
    }

    // 签名行集: 全量口径 (spec §3.R3) — 折叠块锚行照常计入, 在折叠前单点计算。
    let mut signatures: HashSet<usize> = HashSet::new();
    for b in &blocks {
        for f in &b.fragments {
            signatures.insert(f.block.0);
        }
    }

    // 输出层折叠后处理 (spec §3.R): 同规则级联 run 折叠为锚块 + 标记。
    let (blocks, collapse) = collapse_pass(blocks, &noise_lines, params);

    // 统计: output_lines = 幸存块呈现行号去重计数 (noise 剔除后; 折叠省略行
    // 不计 — 呈现口径, spec §3.R3); signature_output_lines 上述全量口径。
    let mut presented: HashSet<usize> = HashSet::new();
    for b in &blocks {
        for l in b.start_line..=b.end_line {
            if !noise_lines.contains(&l) {
                presented.insert(l);
            }
        }
    }
    let stats = Stats {
        input_lines: n,
        output_lines: presented.len(),
        signature_output_lines: signatures.len(),
        noise_lines_removed: noise_lines.len().min(n),
        collapse,
        boundary_violations,
    };
    SkimResult {
        file: file.to_string(),
        input_lines: stats.input_lines,
        output_lines: stats.output_lines,
        blocks,
        stats,
        noise_lines,
        noise_rule_hits,
    }
}

// ---------------------------------------------------------------------------
// 输出层折叠 (spec §3.R)
// ---------------------------------------------------------------------------

/// 块区间 hull (折叠判定用): 该块各 fragment `block` 区间的 min 首 / max 尾
/// — context 扩展不参与 (spec §3.R1: 判定规则命中的语义邻接)。
/// fragments 按行序入块, 但嵌套区间 (后启先闭) 使末元素不必是 max 尾, 须折叠。
fn block_span(b: &OutputBlock) -> (usize, usize) {
    (
        b.fragments
            .iter()
            .map(|f| f.block.0)
            .min()
            .unwrap_or(b.start_line),
        b.fragments
            .iter()
            .map(|f| f.block.1)
            .max()
            .unwrap_or(b.end_line),
    )
}

/// 块的呈现行集注入 (noise 剔除口径, 与 output_lines 同源 — spec §3.R3)。
fn collect_presented(b: &OutputBlock, noise_lines: &HashSet<usize>, out: &mut HashSet<usize>) {
    for l in b.start_line..=b.end_line {
        if !noise_lines.contains(&l) {
            out.insert(l);
        }
    }
}

/// 输出层折叠后处理 (spec §3.R1/R2, 纯函数): 同单规则 + 块间隔 ≤ max_gap 的
/// 极大 run, 长度 ≥ min_repeat 时折叠为首块 (锚块) + CollapseMarker, 被折叠块
/// 从 blocks 移除。返回 (幸存块序列, 审计计数)。经 [skim_with] 消费, 不单独
/// 对外 — 公开参数面是 CollapseParams, 折叠本身是 skim 的内嵌后处理。
///
/// omitted_lines 语义 (spec §3.R2): 全局 = 全部块呈现行集 − 幸存块呈现行集
/// (不变式 output_lines = 未折叠值 − omitted_lines); 每 run 的 marker 值 =
/// 该 run 被折叠块呈现行集 − 幸存块呈现行集 (邻接 context 重叠行由幸存块
/// 呈现时不重复计)。两者独立精确, Σ marker ≥ 全局 (跨 run 重叠时) 不作和。
fn collapse_pass(
    blocks: Vec<OutputBlock>,
    noise_lines: &HashSet<usize>,
    params: &CollapseParams,
) -> (Vec<OutputBlock>, CollapseStats) {
    let effective_min = params.min_repeat.max(2);
    if !params.is_enabled() || blocks.len() < effective_min {
        return (blocks, CollapseStats::default());
    }
    // 块区间预计算 (每块一次; pair 判定与后续消费复用, 不重复折叠 hull)。
    let spans: Vec<(usize, usize)> = blocks.iter().map(block_span).collect();
    // ---- pass 1: 极大 run 划分 ----
    let n = blocks.len();
    let mut runs: Vec<(usize, usize)> = Vec::new(); // [start_idx, end_idx] 含端点
    let mut i = 0;
    while i < n {
        if blocks[i].rules.len() != 1 {
            i += 1;
            continue; // 多规则块永不参与, 同时隔断两侧 run (spec §3.R1)
        }
        let rule = blocks[i].rules[0].as_str();
        let mut j = i;
        while j + 1 < n {
            let nxt = &blocks[j + 1];
            // gap = next_blk_start − cur_blk_end − 1; 单规则块的 fragment
            // 非嵌套递增 (同规则 span 不相交), 相邻输出块 gap ≥ 1 — 无下溢。
            let gap = spans[j + 1].0 - spans[j].1 - 1;
            if nxt.rules.len() == 1 && nxt.rules[0] == rule && gap <= params.max_gap {
                j += 1;
            } else {
                break;
            }
        }
        if j - i + 1 >= effective_min {
            runs.push((i, j));
        }
        i = j + 1;
    }
    if runs.is_empty() {
        return (blocks, CollapseStats::default());
    }
    // ---- pass 2: 呈现行集与 omitted 结算 ----
    let mut is_folded = vec![false; n];
    for (s, e) in &runs {
        for f in is_folded.iter_mut().take(*e + 1).skip(s + 1) {
            *f = true;
        }
    }
    let mut survivor_set: HashSet<usize> = HashSet::new();
    let mut all_set: HashSet<usize> = HashSet::new();
    for (idx, b) in blocks.iter().enumerate() {
        collect_presented(b, noise_lines, &mut all_set);
        if !is_folded[idx] {
            collect_presented(b, noise_lines, &mut survivor_set);
        }
    }
    let omitted: Vec<usize> = runs
        .iter()
        .map(|(s, e)| {
            let mut fset = HashSet::new();
            for b in &blocks[s + 1..=*e] {
                collect_presented(b, noise_lines, &mut fset);
            }
            fset.difference(&survivor_set).count()
        })
        .collect();
    let collapse_stats = CollapseStats {
        folded_blocks: runs.iter().map(|(s, e)| e - s).sum(),
        omitted_lines: all_set.difference(&survivor_set).count(),
    };
    // ---- pass 3 (所有权, 逆序 drain 保索引): 组装标记并移除被折叠块 ----
    let mut blocks = blocks;
    for (k, (s, e)) in runs.iter().enumerate().rev() {
        let folded_blocks: Vec<OutputBlock> = blocks.drain(s + 1..=*e).collect();
        let mut frag_spans = Vec::new();
        let mut start_line = usize::MAX;
        let mut end_line = 0;
        for b in &folded_blocks {
            start_line = start_line.min(b.start_line);
            end_line = end_line.max(b.end_line);
            for f in &b.fragments {
                frag_spans.push(f.block);
            }
        }
        let anchor = &mut blocks[*s];
        anchor.collapse = Some(CollapseMarker {
            rule: anchor.rules[0].clone(),
            intent: anchor.fragments[0].intent,
            repeat_blocks: folded_blocks.len(),
            start_line,
            end_line,
            omitted_lines: omitted[k],
            frag_spans,
        });
    }
    (blocks, collapse_stats)
}

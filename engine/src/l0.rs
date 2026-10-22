//! L0 全语料回归门禁 (口径与判定 SSOT) — spec §4。
//!
//! 职责边界: 采集 (collect: 递归遍历语料仓, 单次加载规则跑全量 skim) →
//!   汇总 (summarize: 三层聚合, 口径单点) → 判定 (gate: 断言族 + 双测对比)。
//!   CLI 形态 `log-skimmer l0` (main.rs 接线); bash/jq 时代脚本的等价移植
//!   (l0-collect.sh / l0-summarize.jq / l0-gate.sh 已退役), 判定逻辑自此可单测。
//!
//! 配对语义 (spec §5): 样本 = 日志 (*.log 或压缩形态 *.log.gz) + 配对名同名
//!   *.log.json sidecar — 配对名 = 样本文件名去 .gz 后缀 (X.log.gz ↔ X.log.json),
//!   单侧在场 = 孤儿 — 不进样本计数, 进 Collected.orphans 供 CLI 告警
//!   (issue #46 可见性, 引擎只产出数据不渲染, 是否升级硬错误留需求方裁决);
//!   同配对名 plain 与 gz 双份在场 = 配对歧义 → Err fail-fast (静默择一会
//!   无声吞掉一份样本)。
//!
//! 口径 (spec §4, 与 jq 时代逐条等价):
//!   - 主指标滤除 rerun-through (attempt>1 且 conclusion=success);
//!   - FN 谓词只数 failure/timed_out 家族样本上 failure-intent fragment 数为 0;
//!     cancelled 截断日志不计缺失 (三态);
//!   - FP 分子 = success 样本上 failure-intent fragment 的 block 区间行数,
//!     率分母 = 该聚合 success 样本输入总行数;
//!   - 零块/边界违例全量统计 (不滤 excluded);
//!   - 压缩率 blk/sig = 聚合行数比 (1-Σout/Σin), 非样本内率平均;
//!   - repeat-collapse 全量口径 (spec §3.R3): 折叠块 fragment 经 collapse 标记
//!     的 frag_spans 照常逐条计数 — FN/FP/per_rule_hits 对呈现层折叠透明;
//!     唯一呈现口径变化在 output_lines (压缩率 blk 分子缩小 = 改善方向)。
//!
//! 判定断言族: 语料在场双断言 (样本 ≥50 全样本域; 仓 ≥100 分层 — 样本下限
//!   已过才判, 防语料面静默缩水) / FN=0 (违例 detail = 逐样本清单
//!   Vec<FnDetail>, 红日志直接点名漏检样本) / FP率≤2% (违例 detail 含逐
//!   (规则, 样本) 对清单 Vec<FpDetail> — 点名哪条规则在哪些样本上 FP) /
//!   边界违例=0 / 零块=0;
//!   压缩率防退化 = 双测对比 (base summary 提供 per-repo 基准, 相对退化 >2% 判红;
//!   未提供则单测模式显式声明 — 引擎仓传 base 引擎实测, 规则仓传 base 规则实测,
//!   同语料下纯单变量归因)。
//! 豁免通道 (T-P2b-2, 百仓语料门禁语义): sidecar `gate_exempt: true` 样本跳过
//!   FN/FP率/零块三断言 (不进失败判定), 指标照常计算并进 exempt 组报告 —
//!   未适配仓 (121/124) 扩容不红, P3 逐仓适配后移出豁免 (豁免余量=适配进度)。
//!   不豁免的哨兵 (与规则适配正交): 边界违例断言 (引擎构造不变量, 域=全样本)、
//!   语料在场双断言 (样本 ≥50 + 仓 ≥100, 域=全样本含豁免; 仓维度分层 —
//!   样本下限已过才判, 不对小语料叠报)、压缩率双测 per_repo 域含豁免仓
//!   (同语料同规则下纯引擎变量归因, 豁免≠引擎回归豁免)。
//! 退出码分层 (issue #44, CLI 侧): 采集层语料非法 (目录不存在/零配对样本) →
//!   exit 2 (与规则侧对称), 语料在场样本断言只对 0<n<50 生效 — 定向验证小语料
//!   fail 属预期 (防空转), note 分流「定向验证」与「疑获取链路」两种读法。
//! 正向验证 (issue #19 B, 贡献闭环「规则确实工作」的信心来源): per_rule_hits
//!   恒报告 (定义域=装载规则全集, 零命中即死规则); dead_rules 断言仅在该
//!   规则集全部规则应在语料上生效时开启 (`--require-all-rules-hit`, rules 仓
//!   CI 与贡献者本地; 本仓 ci-merge 默认不开 — 存量规则集在公开语料上有天然
//!   零命中规则, 语料未覆盖的工具)。

use crate::{load_rules, skim, Intent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

// ---- 阈值 (判定 SSOT; 原 l0-gate.sh 头注释的职责迁移至此) ----
/// 语料在场下限: 防空转假绿 (获取链路整体故障时拒判)。50 = 兼容单源环境
/// (rules 仓 CI 仅 corpus 57 对; 主仓双源 67 对) — 防空转语义在 50 与 67 无差。
pub const MIN_SAMPLES: usize = 50;
/// 语料在场仓维度下限 (分层, 样本 ≥MIN_SAMPLES 已过才判): 防语料面**静默缩水**
/// — 获取链路只深挖少数仓时样本数探不到, 仓数是覆盖广度哨兵。100 取当前语料
/// 面的留余下限 (示范项目集 SSOT 124 项, 123 仓产出语料现状);
/// 域 = 全样本含豁免 (与 samples_total 哨兵同域 — 覆盖面与豁免进度正交)。
pub const MIN_REPOS: usize = 100;
pub const FP_MAX_PCT: f64 = 2.0;
pub const REGRESSION_MAX_PCT: f64 = 2.0;
/// FN violation detail 的样本上限: FN 事件通常 1-3 个不截断; 极端多 (规则集
/// 大面积失明) 时防 verdict JSON 膨胀 — 超限仅列前 N 条, 总数进 note。
pub const FN_DETAIL_MAX: usize = 50;
/// FP violation detail 的 (规则, 样本) 对上限: 与 FN_DETAIL_MAX 同理 — 极端
/// 多 (规则集大面积污染 success 样本) 时防 verdict JSON 膨胀, 超限仅列前
/// N 条, 总数进 note。
pub const FP_DETAIL_MAX: usize = 50;

/// FN 谓词 (口径 SSOT, spec §4): failure 家族样本上 failure-intent fragment
/// 数为 0 (完全漏检)。agg 的 fn_samples 计数与 fn_detail 逐样本清单共用此
/// 单点定义 — 两处口径不得漂移 (计数≠清单长度 = 引擎 bug)。
fn is_fn(s: &L0Sample) -> bool {
    (s.conclusion == "failure" || s.conclusion == "timed_out") && s.failure_frags == 0
}

// ---- 采集层 ----

/// 单样本实测记录 (sidecar 元数据 × skim 结果聚合)。
#[derive(Debug, Clone, Serialize)]
pub struct L0Sample {
    pub repo: String,
    pub conclusion: String,
    pub attempt: u32,
    pub sample: String,
    pub excluded: bool,
    /// 门禁豁免 (sidecar gate_exempt, T-P2b-2): 该样本的 FN/FP/零块不进失败
    /// 判定 — 未适配仓的过渡通道, 指标仍进 exempt 组报告 (适配后移出)。
    pub gate_exempt: bool,
    pub input_lines: usize,
    pub output_lines: usize,
    pub signature_output_lines: usize,
    pub failure_frags: u64,
    pub failure_frag_lines: u64,
    pub warning_frags: u64,
    pub info_frags: u64,
    pub zero_block: u32,
    pub boundary_violations: usize,
    pub frags: Vec<FragRec>,
    /// noise 规则闭块计数 (规则 id → 剔除区块数) — 正向验证域覆盖 noise 规则:
    /// noise 不产 fragment, 其命中只能以剔除区块计 (lib SkimResult 旁路数据)。
    pub noise_hits: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FragRec {
    pub rule: String,
    pub intent: Intent,
    pub lines: u64,
}

/// sidecar 的最小投影 (l0 只消费这四个字段; 全量 sidecar schema 见 spec §5)。
/// attempt 为 Option (P0-2): 缺失信号由类型承载 — 统计按 unwrap_or(1) 口径
/// 不变, 缺口聚合计数进 Collected.sidecars_missing_attempt (可见性配套,
/// 渲染归 CLI — 与孤儿警告同族, 不静默)。gate_exempt 带默认值 (T-P2b-2):
/// 旧 sidecar 无字段时兼容为 false — 存量语料语义零漂移。
#[derive(Deserialize)]
struct Sidecar {
    repo: String,
    conclusion: String,
    attempt: Option<u32>,
    #[serde(default)]
    gate_exempt: bool,
}

/// 递归收集语料文件 (路径字典序, 确定性遍历 — 等价 `find | sort`): *.log.json
/// sidecar 与日志样本 (*.log 或 *.log.gz) 分列, 配对判定在 collect (单侧在场
/// = 孤儿, issue #46)。
fn walk_corpus_files(dir: &Path, logs: &mut Vec<PathBuf>, sidecars: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk_corpus_files(&p, logs, sidecars);
        } else {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            if name.ends_with(".log.json") {
                sidecars.push(p);
            } else if name.ends_with(".log.gz") || p.extension().is_some_and(|x| x == "log") {
                logs.push(p);
            }
        }
    }
}

/// 配对缺口 (issue #46 可见性): 样本两件套 (日志 *.log|*.log.gz + 配对名同名
/// *.log.json) 只有一侧在场。引擎只产出数据 (相对语料根路径 + 缺失侧), 告警
/// 渲染归 CLI — 不升级硬错误 (是否 exit 2 留给需求方)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrphanSample {
    pub path: String,
    pub missing: MissingSide,
}

/// 缺失侧 (在场文件的对侧)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MissingSide {
    /// *.log.json 在场, 配对日志 (*.log / *.log.gz) 缺失
    Log,
    /// 日志 (*.log / *.log.gz) 在场, 同名 *.log.json (sidecar) 缺失
    Sidecar,
}

fn orphan(corpus_dir: &Path, present: &Path, missing: MissingSide) -> OrphanSample {
    OrphanSample {
        path: present
            .strip_prefix(corpus_dir)
            .unwrap_or(present)
            .display()
            .to_string(),
        missing,
    }
}

/// 采集产物: 样本记录 × 装载规则全集 id × 孤儿清单 — rule_ids 是 per_rule_hits
/// 的定义域 (零命中规则只有对照装载清单才能显形, issue #19 B); orphans 是
/// 配对缺口的可见性数据 (渲染归 CLI); sidecars_missing_attempt 是 sidecar
/// 缺失 attempt 字段的聚合计数 (P0-2 可见性: 统计按 attempt=1 兜底不中断,
/// 缺口单行警告渲染走 attempt_missing_warning)。
#[derive(Debug, Clone)]
pub struct Collected {
    pub samples: Vec<L0Sample>,
    pub rule_ids: Vec<String>,
    /// 规则 id → tool-id 映射 (健康档案浏览面链接: 目录名 = tool-id; 规则 id
    /// 自身含连字符不可逆推 — SSOT 来自装载结果)。
    pub rule_tools: BTreeMap<String, String>,
    pub orphans: Vec<OrphanSample>,
    pub sidecars_missing_attempt: usize,
}

impl Collected {
    /// sidecar 缺失 attempt 的单行警告文本 (P0-2): None = 无缺口 (N=0 静默,
    /// 警告只对真实缺口发出); 文本必须含 计数 N 与 按 attempt=1 处理 的说明
    /// — spec §5 必带字段的可见性配套, 消除「rerun 排除永不触发」的静默漂移。
    pub fn attempt_missing_warning(&self) -> Option<String> {
        (self.sidecars_missing_attempt > 0).then(|| {
            format!(
                "l0: 警告: {} 个 sidecar 缺失 attempt 字段 \
                 (按 attempt=1 处理; 规范 §5 要求必带, 建议 refetch)",
                self.sidecars_missing_attempt
            )
        })
    }
}

/// 采集全语料: 规则单次加载, 逐样本 skim。非法规则集/语料目录不存在/日志读取
/// 失败/配对名冲突 → Err (fail-fast, 门禁无兜底); 零配对样本 → Ok 空清单
/// (exit 2 决策在 CLI, issue #44); 零块是正常记录 (违例判定在 gate —
/// 结构承诺的失明信号); 孤儿样本 (单侧在场) 跳过不计数, 见 Collected.orphans。
pub fn collect(corpus_dir: &Path, rules_dir: &Path) -> Result<Collected, String> {
    // 语料目录不存在 = 语料非法 (与 load_rules 的规则目录对称, issue #44) —
    // 采集层失败不是质量门禁红, CLI 侧映射 exit 2。
    if !corpus_dir.is_dir() {
        return Err(format!("语料目录不存在: {}", corpus_dir.display()));
    }
    // LoadErrors Display 自带 "规则集非法 (N 处):" 前缀, 直接透传 — 不得再包
    // 同名前缀 (会叠成 "规则集非法: 规则集非法 (1 处):", issue #73b)。
    let rules = load_rules(rules_dir).map_err(|e| e.to_string())?;
    let (mut logs, mut sidecars) = (Vec::new(), Vec::new());
    walk_corpus_files(corpus_dir, &mut logs, &mut sidecars);
    let log_set: HashSet<String> = logs.iter().map(|p| p.display().to_string()).collect();
    let sidecar_set: HashSet<String> = sidecars.iter().map(|p| p.display().to_string()).collect();
    let mut orphans = Vec::new();
    let mut sidecars_missing_attempt = 0usize;
    let mut samples = Vec::with_capacity(sidecars.len());
    for j in &sidecars {
        let sc_str = j.display().to_string();
        let base = sc_str.strip_suffix(".json").unwrap_or(&sc_str).to_string();
        // 配对候选 (spec §5): base (plain) 与 base.gz (压缩形态), 恰一份在场才
        // 无歧义 — 双份在场 = 配对名冲突 fail-fast (静默择一会无声吞掉一份样本)。
        let gz = format!("{base}.gz");
        let plain = base;
        let log_path = match (log_set.contains(&plain), log_set.contains(&gz)) {
            (false, false) => {
                orphans.push(orphan(corpus_dir, j, MissingSide::Log));
                continue;
            }
            (true, true) => {
                return Err(format!(
                    "样本配对名冲突: {plain} 与 {gz} 同配对名 (sidecar {sc_str}), 拒绝静默择一"
                ));
            }
            (true, false) => PathBuf::from(&plain),
            (false, true) => PathBuf::from(&gz),
        };
        let sc: Sidecar = serde_json::from_str(
            &std::fs::read_to_string(j)
                .map_err(|e| format!("sidecar 读取失败 {}: {e}", j.display()))?,
        )
        .map_err(|e| format!("sidecar 解析失败 {}: {e}", j.display()))?;
        let raw = crate::sample_io::read_log_sample(&log_path)
            .map_err(|e| format!("日志读取失败 {e}"))?;
        let lines = crate::normalize_lines(&raw);
        let r = skim(&log_path.display().to_string(), &rules, &lines);
        let sample = log_path
            .strip_prefix(corpus_dir)
            .unwrap_or(&log_path)
            .display()
            .to_string();
        let mut failure_frags = 0u64;
        let mut failure_frag_lines = 0u64;
        let mut warning_frags = 0u64;
        let mut info_frags = 0u64;
        let mut frags = Vec::new();
        // 全量口径恢复 (spec §3.R3): 折叠块 fragment 照常逐条计数 — 折叠是
        // 呈现层概念, FN/FP/per_rule_hits 消费 frag_spans (锚块 rules[0] 同规则,
        // intent 规则级常量), 与未折叠引擎逐值等价。
        let mut record = |rule: &str, intent: Intent, span: (usize, usize)| {
            let lines = (span.1 - span.0 + 1) as u64;
            match intent {
                Intent::Failure => {
                    failure_frags += 1;
                    failure_frag_lines += lines;
                }
                Intent::Warning => warning_frags += 1,
                Intent::Info => info_frags += 1,
            }
            frags.push(FragRec {
                rule: rule.to_string(),
                intent,
                lines,
            });
        };
        for b in &r.blocks {
            for f in &b.fragments {
                record(&f.rule, f.intent, f.block);
            }
            if let Some(m) = &b.collapse {
                for span in &m.frag_spans {
                    record(&m.rule, m.intent, *span);
                }
            }
        }
        // P0-2: 缺失 attempt 按 1 处理 (统计口径不变), 缺口聚合计数不静默。
        let attempt = sc.attempt.unwrap_or(1);
        sidecars_missing_attempt += usize::from(sc.attempt.is_none());
        let excluded = attempt > 1 && sc.conclusion == "success";
        samples.push(L0Sample {
            repo: sc.repo,
            conclusion: sc.conclusion,
            attempt,
            sample,
            excluded,
            gate_exempt: sc.gate_exempt,
            input_lines: r.input_lines,
            output_lines: r.output_lines,
            signature_output_lines: r.stats.signature_output_lines,
            failure_frags,
            failure_frag_lines,
            warning_frags,
            info_frags,
            zero_block: u32::from(r.blocks.is_empty()),
            boundary_violations: r.stats.boundary_violations.len(),
            frags,
            noise_hits: r.noise_rule_hits,
        });
    }
    for l in &logs {
        // 配对名 = 样本文件名去 .gz 后缀 (X.log.gz → X.log); sidecar 恒为 配对名.json。
        let l_str = l.display().to_string();
        let base = l_str.strip_suffix(".gz").unwrap_or(&l_str);
        if !sidecar_set.contains(&format!("{base}.json")) {
            orphans.push(orphan(corpus_dir, l, MissingSide::Sidecar));
        }
    }
    orphans.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Collected {
        samples,
        rule_ids: rules.iter().map(|r| r.id.clone()).collect(),
        rule_tools: rules
            .iter()
            .map(|r| (r.id.clone(), r.tool.clone()))
            .collect(),
        orphans,
        sidecars_missing_attempt,
    })
}

// ---- 汇总层 ----

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Agg {
    pub runs: usize,
    pub input_lines: u64,
    pub output_lines: u64,
    pub signature_output_lines: u64,
    #[serde(rename = "fn")]
    pub fn_samples: usize,
    pub success_input_lines: u64,
    pub fp_frags: u64,
    pub fp_lines: u64,
    pub success_warning_frags: u64,
    pub success_info_frags: u64,
    pub comp_blk_pct: f64,
    pub comp_sig_pct: f64,
    pub fp_rate_pct: f64,
}

fn ratio_pct(num: f64, den: f64) -> f64 {
    if den > 0.0 {
        100.0 * num / den
    } else {
        0.0
    }
}

/// 两位小数收敛 (issue #74): 压缩率百分比的 f64 全精度尾巴 (98.349999…)
/// 在 stdout JSON 与 stderr 摘要两侧均无信息量 — 于计算点单点收敛, 数值类型
/// 不变 (消费方 `--base` 走 serde 数值解析, 双测对比两侧同点收敛)。
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn agg(samples: &[&L0Sample]) -> Agg {
    let mut a = Agg {
        runs: samples.len(),
        ..Default::default()
    };
    for s in samples {
        a.input_lines += s.input_lines as u64;
        a.output_lines += s.output_lines as u64;
        a.signature_output_lines += s.signature_output_lines as u64;
        if is_fn(s) {
            a.fn_samples += 1;
        }
        if s.conclusion == "success" {
            a.success_input_lines += s.input_lines as u64;
            a.fp_frags += s.failure_frags;
            a.fp_lines += s.failure_frag_lines;
            a.success_warning_frags += s.warning_frags;
            a.success_info_frags += s.info_frags;
        }
    }
    a.comp_blk_pct = round2(ratio_pct(
        a.input_lines as f64 - a.output_lines as f64,
        a.input_lines as f64,
    ));
    a.comp_sig_pct = round2(ratio_pct(
        a.input_lines as f64 - a.signature_output_lines as f64,
        a.input_lines as f64,
    ));
    a.fp_rate_pct = ratio_pct(a.fp_lines as f64, a.success_input_lines as f64);
    a
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleAgg {
    pub frags: usize,
    pub lines: u64,
    pub fp_frags: usize,
    pub hit_repos: usize,
}

// ---- 规则健康档案 (2026-10-21 百仓调研落地产物) ----
//
// 语义 SSOT (信息论推导, 实测断裂带背书 — 断崖 98.8% ↔ ≤4.5%, 阈值在断裂带内
// 任意取值判定不变, 分布随 latest.md 发布可审计):
//   a = 失败族样本命中数 (样本级, ≥1 fragment 即命中, 非 fragment 计数)
//   b = 成功样本命中数
//   ppv     = a/(a+b)              P(失败|触发) — 触发推出结论的可靠度
//   ppv_lo  = Wilson 95% 下界      小样本下健全性的保守证明
//   lift    = ppv / 失败基率        与结论的相关性 (1.0 = 独立, 基率随槽位比浮动,
//                                   判独立性用 lift 不用绝对 ppv — 采样比鲁棒)
//   spec_bits = -log2(a/N_f)       触发携带的样本信息量 (选择性; 0 = 处处触发零信息)
//
// 分级 (grade):
//   restating   P_f ≥ 0.8 ∧ P_s ≤ 0.05 — 触发由结论决定而非内容 (注解地板形态):
//               高可靠零信息, 计入零块防线不计入诊断覆盖
//   diagnostic  P_f ≤ 0.2 — 触发由内容决定 (≥2.3 比特/块), 计入诊断覆盖
//   boundary    其余 (含 a=0∧b>0 的纯成功异常) — 分布中不应存在 (断裂带空无一
//               物), 出现即列名人工审; 默认按 diagnostic 计入覆盖 (保守方向是
//               高估覆盖, FN/FP 门禁仍是硬警察)
//
// 诊断覆盖率 (diagnostic_coverage) = 含 ≥1 诊断规则失败块的失败族样本 / 失败族
// 样本 — 软指标 (报告面, 不入门禁断言): FN 保证"有块", 诊断覆盖保证"块由内容
// 决定" (地板块的 ##[error]Process completed with exit code 1. 形态零诊断信息)。
// intent 的统计可分性 (实测): {failure,warning}=证据类 vs {info}=快照类 (lift≈1
// ∧ 近常数触发) 可分; failure↔warning 统计同签名 (warning 与失败强相关), 属内
// 容语义 — 健康档案验证声明而非替代标注。

/// 分级阈值 (断裂带内取值, 见上注释; 测试锁定判定不变性)。
pub const RESTATE_PF_MIN: f64 = 0.8;
pub const RESTATE_PS_MAX: f64 = 0.05;
pub const DIAG_PF_MAX: f64 = 0.2;

/// 每规则触发计数 (样本级) — 健康档案基数。域 = main (滤 rerun-through, 与
/// per_rule 同域); noise 规则不产 fragment 天然缺席 (liveness 归 per_rule_hits)。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RuleFire {
    pub fire_failure: u32,
    pub fire_success: u32,
}

/// 规则分级 (serde 小写字符串 — 报告/徽章消费)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleGrade {
    Restating,
    Diagnostic,
    Boundary,
}

/// 每规则健康档案 (从 RuleFire + 样本总量推导的纯函数)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleHealth {
    pub ppv: f64,
    pub ppv_lo: f64,
    pub lift: f64,
    pub spec_bits: f64,
    pub grade: RuleGrade,
    /// 声明意图 (fragment 首见 intent; 快照性核对: info 应 lift≈1.0)。
    pub intent: Intent,
    /// 工具目录名 (浏览面链接用; 规则 id 含连字符不可逆推目录)。
    pub tool: String,
}

/// 诊断覆盖率 (软指标, 见模块注释)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagCoverage {
    pub covered: usize,
    pub total: usize,
    pub pct: f64,
}

/// Wilson 95% 置信下界 (单比例, z=1.96): 小样本下 "PPV=1.0" 的保守化。
pub fn wilson_lo(a: f64, b: f64) -> f64 {
    let n = a + b;
    if n <= 0.0 {
        return 0.0;
    }
    let z = 1.96_f64;
    let p = a / n;
    let lo = (p + z * z / (2.0 * n) - z * ((p * (1.0 - p) + z * z / (4.0 * n)) / n).sqrt())
        / (1.0 + z * z / n);
    lo.max(0.0)
}

/// 单规则健康推导 (纯函数, 测试直测): 基率 = n_f/(n_f+n_s), 由调用方给
/// (与整批共享同一基率)。
pub fn rule_health(
    rf: RuleFire,
    n_f: usize,
    n_s: usize,
    base_rate: f64,
    intent: Intent,
    tool: String,
) -> RuleHealth {
    let (a, b) = (rf.fire_failure as f64, rf.fire_success as f64);
    let n = a + b;
    let ppv = if n > 0.0 { a / n } else { 0.0 };
    let p_f = if n_f > 0 { a / n_f as f64 } else { 0.0 };
    let p_s = if n_s > 0 { b / n_s as f64 } else { 0.0 };
    let grade = if a == 0.0 && b > 0.0 {
        // 纯成功触发 = FP 形态异常, 进人工审桶 (ppv=0/lift=0 明示)
        RuleGrade::Boundary
    } else if p_f >= RESTATE_PF_MIN && p_s <= RESTATE_PS_MAX {
        RuleGrade::Restating
    } else if p_f <= DIAG_PF_MAX {
        RuleGrade::Diagnostic
    } else {
        RuleGrade::Boundary
    };
    RuleHealth {
        ppv: round2(ppv * 1000.0) / 1000.0,
        ppv_lo: round2(wilson_lo(a, b) * 1000.0) / 1000.0,
        lift: if base_rate > 0.0 {
            round2(ppv / base_rate * 100.0) / 100.0
        } else {
            0.0
        },
        spec_bits: if a > 0.0 {
            round2(-(a / n_f as f64).log2())
        } else {
            0.0
        },
        grade,
        intent,
        tool,
    }
}

/// 健康档案整批计算 (summarize 内部 + 测试直测): 输入 main 域样本, 产出
/// (per_rule_fire, per_rule_health, diagnostic_coverage) 三元组。
pub fn health(
    samples: &[&L0Sample],
    rule_tools: &BTreeMap<String, String>,
) -> (
    BTreeMap<String, RuleFire>,
    BTreeMap<String, RuleHealth>,
    DiagCoverage,
) {
    let failure_family = |s: &L0Sample| s.conclusion == "failure" || s.conclusion == "timed_out";
    let n_f = samples.iter().filter(|s| failure_family(s)).count();
    let n_s = samples.iter().filter(|s| s.conclusion == "success").count();
    let base_rate = if n_f + n_s > 0 {
        n_f as f64 / (n_f + n_s) as f64
    } else {
        0.0
    };
    let mut fire: BTreeMap<String, RuleFire> = BTreeMap::new();
    let mut intents: BTreeMap<String, Intent> = BTreeMap::new();
    for &s in samples {
        let mut seen: HashSet<&str> = HashSet::new();
        for f in &s.frags {
            if !seen.insert(f.rule.as_str()) {
                continue; // 样本级去重: 一样本多 fragment 只计一次触发
            }
            let e = fire.entry(f.rule.clone()).or_default();
            if failure_family(s) {
                e.fire_failure += 1;
            } else if s.conclusion == "success" {
                e.fire_success += 1;
            }
            intents.entry(f.rule.clone()).or_insert(f.intent);
        }
    }
    let health_map: BTreeMap<String, RuleHealth> = fire
        .iter()
        .map(|(rid, rf)| {
            (
                rid.clone(),
                rule_health(
                    *rf,
                    n_f,
                    n_s,
                    base_rate,
                    *intents.get(rid).unwrap_or(&Intent::Failure),
                    rule_tools.get(rid).cloned().unwrap_or_default(),
                ),
            )
        })
        .collect();
    // 诊断覆盖: 失败族样本含 ≥1 诊断规则的 failure fragment
    let diag: HashSet<&str> = health_map
        .iter()
        .filter(|(_, h)| h.grade != RuleGrade::Restating)
        .map(|(k, _)| k.as_str())
        .collect();
    let covered = samples
        .iter()
        .filter(|s| failure_family(s))
        .filter(|s| {
            s.frags
                .iter()
                .any(|f| f.intent == Intent::Failure && diag.contains(f.rule.as_str()))
        })
        .count();
    let coverage = DiagCoverage {
        covered,
        total: n_f,
        pct: round2(ratio_pct(covered as f64, n_f as f64)),
    };
    (fire, health_map, coverage)
}

/// 断言面哨兵聚合 (域分层, T-P2b-2): `zero_block` = gated 样本 (含
/// rerun-through, 豁免不计 — 零块断言语义); `exempt_zero_block`/`exempt_boundary`
/// = 豁免样本镜像 (报告面); `boundary_violations` = 全样本 (引擎构造不变量,
/// 豁免不豁免引擎 bug); `samples_*` = 全样本计数 (语料在场哨兵含豁免 —
/// 防空转全覆盖; `samples_exempt` = 豁免余量/适配进度度量)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Structural {
    pub zero_block: u32,
    pub boundary_violations: usize,
    pub samples_total: usize,
    pub samples_excluded: usize,
    #[serde(default)]
    pub exempt_zero_block: u32,
    #[serde(default)]
    pub exempt_boundary: usize,
    #[serde(default)]
    pub samples_exempt: usize,
    /// 有 ≥1 个豁免主指标样本的仓数 (ANY 语义 — 混合仓也计)。
    #[serde(default)]
    pub exempt_repos: usize,
    /// 去重仓数 (全样本域含豁免) — 语料在场仓维度哨兵 (`corpus-repos-missing`
    /// 断言的数据面, 分层: 样本下限已过才判)。serde 默认 0: 旧 base summary
    /// 无此字段仍可解析 (断言域在当前侧, 默认值不参与判定)。
    #[serde(default)]
    pub repos_total: usize,
}

/// 逐仓分解行 (sweep 报告骨架, T-P2b-2): 样本数/FN/FP/零块/压缩率 + 豁免
/// 样本数。行口径 = 主指标域 (滤 rerun-through), FN/FP 不分 gated/exempt
/// (诊断视图 — 未适配仓离达标多远一眼可见); `exempt_samples` 标组属 (0 = 该仓
/// 全入断言)。例外: 零块/边界为**全域口径** (含 rerun-through, 与组级哨兵
/// 同域 — 行和 = gated + exempt 组值)。断言面组值以 totals/structural 为准
/// (混合仓行值 ≠ 组值和)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoRow {
    pub runs: usize,
    pub exempt_samples: usize,
    pub fn_samples: usize,
    pub fp_rate_pct: f64,
    pub fp_lines: u64,
    pub zero_block: u32,
    pub boundary_violations: usize,
    pub comp_blk_pct: f64,
    pub comp_sig_pct: f64,
}

/// FN violation detail 逐样本条目: 点名一个完全漏检的失败样本 (repo + 相对
/// 语料根路径 + 输入行数) — 红日志可读性 + fn-responder 免全语料扫描定位。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FnDetail {
    pub repo: String,
    pub sample: String,
    pub input_lines: u64,
}

/// FP violation detail 逐对条目: 点名一组 success 样本上的 failure-intent
/// 假阳性 (规则 + 样本定位 + 幅度) — 与 FnDetail 对称, 迭代环 agent 调规则
/// (收紧 pattern) 的机读定位。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FpDetail {
    pub rule: String,
    pub repo: String,
    pub sample: String,
    /// 该 (规则, 样本) 对的 failure-intent fragment 数。
    pub fp_frags: u64,
    /// 该对的 block 区间行数合计 (FP 率分子口径的单对分解)。
    pub fp_lines: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    /// repo|conclusion 单样本聚合, 域 = gated 主指标样本 (报告面)。
    pub per_repo_conclusion: BTreeMap<String, Agg>,
    /// 逐仓聚合, 域 = 主指标样本 (gated + exempt 全部仓, 滤 rerun-through) —
    /// 压缩率双测对比的比对域 (引擎变量归因, 豁免仓不豁免引擎回归)。
    pub per_repo: BTreeMap<String, Agg>,
    /// gated 组聚合 — gate() FN/FP 断言的判定输入 (无豁免语料下与历史语义
    /// 逐值等价)。
    pub totals: Agg,
    pub per_rule: BTreeMap<String, RuleAgg>,
    pub structural: Structural,
    /// 每规则命中计数 (正向验证, issue #19 B): 定义域 = 装载规则全集;
    /// signal = fragment 数 (存活 fragment — A6 下被 noise 全覆盖的 signal
    /// 闭块被丢弃, 不计), noise = 剔除区块数; **全样本域** (含 excluded —
    /// liveness 语义, 仅被 rerun 样本命中不是死规则)。serde(default): base
    /// summary 产自旧引擎时无此字段, 判定侧只消费 PR 侧的值。
    #[serde(default)]
    pub per_rule_hits: BTreeMap<String, u64>,
    /// exempt 组聚合 (报告面, 不进断言) — 与 totals 同构镜像。
    #[serde(default)]
    pub exempt_totals: Agg,
    /// 逐仓分解表 (sweep 报告骨架, 124 仓规模) — 见 RepoRow 行口径。
    #[serde(default)]
    pub per_repo_breakdown: BTreeMap<String, RepoRow>,
    /// per-rule × per-repo 命中矩阵 (T-P2b-2): 定义域 = 装载规则全集 (零命中
    /// 规则 = 空 map), 计数口径与 per_rule_hits 同 (signal fragment ∪ noise
    /// 剔除区块, 全样本域 liveness); 消费面 = projects.yaml tools 预期标签
    /// 真值化 + 死规则/盲区发现 (规则在哪些仓命中、哪些仓对哪些规则全盲)。
    #[serde(default)]
    pub rule_repo_hits: BTreeMap<String, BTreeMap<String, u64>>,
    /// 每规则触发计数 (样本级, 健康档案基数) — 见 health() 模块注释。
    /// serde(default): 旧 base summary 产自旧引擎时无此字段, 健康档案只消费
    /// 当前侧 (双测对比不涉及), base 侧缺省无碍。
    #[serde(default)]
    pub per_rule_fire: BTreeMap<String, RuleFire>,
    /// 每规则健康档案 (ppv/ppv_lo/lift/spec_bits/grade/intent)。
    #[serde(default)]
    pub per_rule_health: BTreeMap<String, RuleHealth>,
    /// 诊断覆盖率 (软指标, 报告面): 见 health() 模块注释。
    #[serde(default)]
    pub diagnostic_coverage: DiagCoverage,
    /// FN violation detail 数据源: 逐 FN 样本清单 (repo/相对路径/输入行数),
    /// 域与谓词 = totals.fn_samples (gated 主指标, is_fn 单点) — 豁免仓的 FN
    /// 不进断言细节, rerun-through 滤除。serde(default): 旧 base summary 无此
    /// 字段仍可解析 (gate 的 FN detail 只消费当前侧, 双测不涉及)。
    #[serde(default)]
    pub fn_detail: Vec<FnDetail>,
    /// FP violation detail 数据源: gated 域 success 样本上 failure-intent
    /// fragment 的 (规则, 样本) 对清单 — 口径与 agg 的 FP 分子同谓词
    /// (Intent::Failure × conclusion=success), 豁免仓的 FP 不进断言细节。
    /// serde(default) 兼容同 fn_detail。
    #[serde(default)]
    pub fp_detail: Vec<FpDetail>,
}

/// 样本三域划分 (T-P2b-2): main = 全样本滤 rerun-through; gated = main 滤
/// gate_exempt (断言面); exempt = main 中 gate_exempt (报告面)。豁免样本的
/// rerun-through 照常滤除 (exempt 组内不双计重跑 attempt)。
pub fn summarize(c: &Collected) -> Summary {
    let all = &c.samples;
    let main: Vec<&L0Sample> = all.iter().filter(|s| !s.excluded).collect();
    let gated: Vec<&L0Sample> = main.iter().copied().filter(|s| !s.gate_exempt).collect();
    let exempt: Vec<&L0Sample> = main.iter().copied().filter(|s| s.gate_exempt).collect();
    let mut per_repo_conclusion = BTreeMap::new();
    for &s in &gated {
        per_repo_conclusion.insert(format!("{}|{}", s.repo, s.conclusion), agg(&[s]));
    }
    // FN violation detail 逐样本清单 (gated 域 — 豁免仓的 FN 不进断言细节;
    // 谓词与 agg 计数共用 is_fn 单点, 清单长度 ≡ totals.fn_samples)。
    let fn_detail: Vec<FnDetail> = gated
        .iter()
        .filter(|s| is_fn(s))
        .map(|s| FnDetail {
            repo: s.repo.clone(),
            sample: s.sample.clone(),
            input_lines: s.input_lines as u64,
        })
        .collect();
    // FP violation detail 逐 (规则, 样本) 对清单 (gated 域 — 豁免仓的 FP 不进
    // 断言细节): success 样本上 failure-intent fragment 按规则聚合; 样本间
    // 扫描序 + 样本内规则 id 序 (BTreeMap), 全序确定。
    let mut fp_detail: Vec<FpDetail> = Vec::new();
    for &s in &gated {
        if s.conclusion != "success" {
            continue;
        }
        let mut by_rule: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for f in &s.frags {
            if f.intent == Intent::Failure {
                let (frags, lines) = by_rule.entry(f.rule.as_str()).or_default();
                *frags += 1;
                *lines += f.lines;
            }
        }
        fp_detail.extend(
            by_rule
                .into_iter()
                .map(|(rule, (fp_frags, fp_lines))| FpDetail {
                    rule: rule.to_string(),
                    repo: s.repo.clone(),
                    sample: s.sample.clone(),
                    fp_frags,
                    fp_lines,
                }),
        );
    }
    // 全样本按仓分组 (单一分组源): per_repo (主指标域 Agg) 与逐仓表 (全域
    // 零块/边界 + 主指标域其余行值) 共用 — 分组逻辑单点。
    let mut per_repo_all: BTreeMap<String, Vec<&L0Sample>> = BTreeMap::new();
    for s in all {
        per_repo_all.entry(s.repo.clone()).or_default().push(s);
    }
    let per_repo: BTreeMap<String, Agg> = per_repo_all
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                agg(&v
                    .iter()
                    .copied()
                    .filter(|s| !s.excluded)
                    .collect::<Vec<_>>()),
            )
        })
        .collect();
    let mut per_rule: BTreeMap<String, Vec<(Intent, u64, String, String)>> = BTreeMap::new();
    for &s in &main {
        for f in &s.frags {
            per_rule.entry(f.rule.clone()).or_default().push((
                f.intent,
                f.lines,
                s.repo.clone(),
                s.conclusion.clone(),
            ));
        }
    }
    let per_rule: BTreeMap<String, RuleAgg> = per_rule
        .into_iter()
        .map(|(rule, recs)| {
            (
                rule,
                RuleAgg {
                    frags: recs.len(),
                    lines: recs.iter().map(|(_, l, _, _)| *l).sum(),
                    fp_frags: recs
                        .iter()
                        .filter(|(i, _, _, c)| *i == Intent::Failure && *c == "success")
                        .count(),
                    hit_repos: recs
                        .iter()
                        .filter(|(_, _, _, c)| c != "success")
                        .map(|(_, _, r, _)| r.clone())
                        .collect::<Vec<_>>()
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len(),
                },
            )
        })
        .collect();
    let totals = agg(&gated);
    let exempt_totals = agg(&exempt);
    let per_repo_breakdown: BTreeMap<String, RepoRow> = per_repo_all
        .iter()
        .map(|(repo, samples)| {
            let main_repo: Vec<&L0Sample> =
                samples.iter().copied().filter(|s| !s.excluded).collect();
            let a = agg(&main_repo);
            (
                repo.clone(),
                RepoRow {
                    runs: a.runs,
                    exempt_samples: main_repo.iter().filter(|s| s.gate_exempt).count(),
                    fn_samples: a.fn_samples,
                    fp_rate_pct: a.fp_rate_pct,
                    fp_lines: a.fp_lines,
                    zero_block: samples.iter().map(|s| s.zero_block).sum(),
                    boundary_violations: samples.iter().map(|s| s.boundary_violations).sum(),
                    comp_blk_pct: a.comp_blk_pct,
                    comp_sig_pct: a.comp_sig_pct,
                },
            )
        })
        .collect();
    let repos_total = all
        .iter()
        .map(|s| s.repo.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let structural = Structural {
        zero_block: all
            .iter()
            .filter(|s| !s.gate_exempt)
            .map(|s| s.zero_block)
            .sum(),
        exempt_zero_block: all
            .iter()
            .filter(|s| s.gate_exempt)
            .map(|s| s.zero_block)
            .sum(),
        boundary_violations: all.iter().map(|s| s.boundary_violations).sum(),
        exempt_boundary: all
            .iter()
            .filter(|s| s.gate_exempt)
            .map(|s| s.boundary_violations)
            .sum(),
        samples_total: all.len(),
        samples_excluded: all.iter().filter(|s| s.excluded).count(),
        samples_exempt: all.iter().filter(|s| s.gate_exempt).count(),
        exempt_repos: exempt
            .iter()
            .map(|s| s.repo.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        repos_total,
    };
    // 正向验证: 装载全集补零 + 全样本域累计 (frags ∪ noise_hits, 二者按规则
    // kind 天然不相交 — signal 不进 noise_hits, noise 不产 frags)。
    let mut per_rule_hits: BTreeMap<String, u64> =
        c.rule_ids.iter().map(|id| (id.clone(), 0)).collect();
    for s in all {
        for f in &s.frags {
            *per_rule_hits.entry(f.rule.clone()).or_default() += 1;
        }
        for (id, n) in &s.noise_hits {
            *per_rule_hits.entry(id.clone()).or_default() += n;
        }
    }
    // per-rule × per-repo 矩阵: 与 per_rule_hits 同口径 (liveness 全样本域,
    // 定义域 = 装载全集补空 map), 加 repo 维度。
    let mut rule_repo_hits: BTreeMap<String, BTreeMap<String, u64>> = c
        .rule_ids
        .iter()
        .map(|id| (id.clone(), BTreeMap::new()))
        .collect();
    for s in all {
        for f in &s.frags {
            *rule_repo_hits
                .entry(f.rule.clone())
                .or_default()
                .entry(s.repo.clone())
                .or_default() += 1;
        }
        for (id, n) in &s.noise_hits {
            // 零计数不落格 (skim 对 noise 规则恒报条目含 0): 矩阵保持稀疏,
            // 格子缺位 = 该仓零命中 — 与 signal 侧 (仅命中产 fragment) 对齐。
            if *n > 0 {
                *rule_repo_hits
                    .entry(id.clone())
                    .or_default()
                    .entry(s.repo.clone())
                    .or_default() += n;
            }
        }
    }
    // 健康档案三元组 (main 域 — 滤 rerun-through, 与 per_rule 同域)
    let (per_rule_fire, per_rule_health, diagnostic_coverage) = health(&main, &c.rule_tools);
    Summary {
        per_repo_conclusion,
        per_repo,
        totals,
        per_rule,
        structural,
        per_rule_hits,
        exempt_totals,
        per_repo_breakdown,
        rule_repo_hits,
        per_rule_fire,
        per_rule_health,
        diagnostic_coverage,
        fn_detail,
        fp_detail,
    }
}

// ---- 判定层 ----

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub note: String,
    /// 违例定位细节 (形态随 kind): fn → Vec<FnDetail> 逐 FN 样本清单 (旧
    /// verdict 空数组 `[]` 可直接反序列化为空 Vec — 类型兼容); fp-rate →
    /// {fp_rate_pct, fp_lines, fp_detail: Vec<FpDetail>} 对象 (逐 (规则,
    /// 样本) 对定位); dead-rules → 规则 id 数组; 其余 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Regression {
    pub repo: String,
    pub delta_blk_pct: f64,
    pub cur_blk_pct: f64,
    pub base_blk_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompressionCompare {
    pub mode: &'static str,
    pub regressions: Vec<Regression>,
    pub drift_notices: Vec<String>,
    pub base_fail: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    pub verdict: &'static str,
    pub stats: serde_json::Value,
    pub violations: Vec<Violation>,
    pub compression_compare: CompressionCompare,
}

/// 组聚合 JSON 视图 (报告双组节): gated 组的边界值 = 全域减豁免域
/// (saturating — 手工构造的 summary 不保证子集关系); 断言面仍读
/// structural.boundary_violations 全域值 (见 Verdict::exempt_line 注)。
fn group_stats_json(a: &Agg, zero_block: u32, boundary: usize) -> serde_json::Value {
    serde_json::json!({
        "runs": a.runs,
        "fn": a.fn_samples,
        "fp_rate_pct": a.fp_rate_pct,
        "fp_lines": a.fp_lines,
        "zero_block": zero_block,
        "boundary_violations": boundary,
        "comp_blk_pct": a.comp_blk_pct,
        "comp_sig_pct": a.comp_sig_pct,
    })
}

/// 判定入口: 断言族 (FN/FP率/零块对 gated 组; 边界/语料在场对全样本) +
/// 双测压缩率对比 (base 在场时)。
/// `require_all_rules_hit` = 正向验证断言 (issue #19 B): 开启时存在语料零命中
/// 规则 (dead_rules) → fail。默认关闭 — verdict 语义不变; 该断言供规则集
/// 全体规则均应在语料上生效的场景 (rules 仓 CI / 贡献者本地验证)。
pub fn gate(pr: &Summary, base: Option<&Summary>, require_all_rules_hit: bool) -> Verdict {
    let mut violations = Vec::new();
    let t = &pr.totals;
    let st = &pr.structural;
    let dead_rules: Vec<String> = pr
        .per_rule_hits
        .iter()
        .filter(|(_, h)| **h == 0)
        .map(|(id, _)| id.clone())
        .collect();
    if st.samples_total < MIN_SAMPLES {
        violations.push(Violation {
            kind: "corpus-missing",
            note: format!(
                "语料样本数不足 ({} <{MIN_SAMPLES}) — 定向验证小语料属预期 \
                 (fail 为防空转设计, 正向信号以 per_rule_hits 为准); \
                 全量语料下则疑获取链路故障",
                st.samples_total
            ),
            detail: None,
        });
    }
    // 仓维度哨兵 (分层): 样本下限已过才判 — 样本不足场景已由 corpus-missing
    // 报告, 本断言专责「样本深度够但覆盖广度缩水」形态, 不对小语料叠报。
    if st.samples_total >= MIN_SAMPLES && st.repos_total < MIN_REPOS {
        violations.push(Violation {
            kind: "corpus-repos-missing",
            note: format!(
                "语料面静默缩水: 样本 {}≥{MIN_SAMPLES} 但去重仓数 {}<{MIN_REPOS} — \
                 覆盖广度不足 (获取链路只深挖少数仓的形态, 样本维度哨兵探不到); \
                 定向验证请以 per_rule_hits 为信号",
                st.samples_total, st.repos_total
            ),
            detail: None,
        });
    }
    if t.fn_samples > 0 {
        // 红日志点名样本: detail = 逐 FN 样本清单 (Vec<FnDetail>, serde 兼容 —
        // 旧 verdict 的空数组可反序列化为空 Vec)。FN 事件通常 1-3 个不截断;
        // 极端多时上限 FN_DETAIL_MAX, 总数进 note (fn-responder 免全语料扫描)。
        let listed: Vec<FnDetail> = pr.fn_detail.iter().take(FN_DETAIL_MAX).cloned().collect();
        let note = if pr.fn_detail.len() > FN_DETAIL_MAX {
            format!(
                "失败样本零 failure-intent fragment (完全漏检, 召回优先不变量) \
                 — FN 样本 {} 个, detail 上限 {FN_DETAIL_MAX} 仅列前 {FN_DETAIL_MAX}",
                t.fn_samples
            )
        } else {
            "失败样本零 failure-intent fragment (完全漏检, 召回优先不变量)".into()
        };
        violations.push(Violation {
            kind: "fn",
            note,
            detail: Some(serde_json::json!(listed)),
        });
    }
    if t.fp_rate_pct > FP_MAX_PCT {
        // 红日志点名 (规则, 样本) 对 (与 fn violation 对称): fp_detail 逐对
        // 定位 "哪条规则在哪些样本上 FP"; 极端多时上限 FP_DETAIL_MAX, 总数进 note。
        let listed: Vec<FpDetail> = pr.fp_detail.iter().take(FP_DETAIL_MAX).cloned().collect();
        let note = if pr.fp_detail.len() > FP_DETAIL_MAX {
            format!(
                "success 样本 failure-intent 假阳性率超阈 ({:.2}% >{FP_MAX_PCT}%) \
                 — FP 定位对 {} 个, detail 上限 {FP_DETAIL_MAX} 仅列前 {FP_DETAIL_MAX}",
                t.fp_rate_pct,
                pr.fp_detail.len()
            )
        } else {
            format!(
                "success 样本 failure-intent 假阳性率超阈 ({:.2}% >{FP_MAX_PCT}%)",
                t.fp_rate_pct
            )
        };
        violations.push(Violation {
            kind: "fp-rate",
            note,
            detail: Some(serde_json::json!({
                "fp_rate_pct": t.fp_rate_pct,
                "fp_lines": t.fp_lines,
                "fp_detail": listed,
            })),
        });
    }
    if st.boundary_violations > 0 {
        violations.push(Violation {
            kind: "boundary",
            note: "block 尾行未被该规则任一 pattern 匹配 (A4 边界完整性哨兵, \
                   按构造恒真 — 非零 = 引擎 bug)"
                .into(),
            detail: None,
        });
    }
    if st.zero_block > 0 {
        violations.push(Violation {
            kind: "zero-block",
            note: "存在 skim 零块样本 — 规则集对该日志失明 (结构承诺)".into(),
            detail: None,
        });
    }
    if require_all_rules_hit && !dead_rules.is_empty() {
        violations.push(Violation {
            kind: "dead-rules",
            note: format!(
                "存在语料零命中规则 {} 条 (正向验证: 拼错的正则不得全绿合入, issue #19)",
                dead_rules.len()
            ),
            detail: Some(serde_json::json!(dead_rules)),
        });
    }

    let cmp = match base {
        Some(b) => {
            let mut regressions = Vec::new();
            let mut drifts = Vec::new();
            for (repo, cur) in &pr.per_repo {
                match b.per_repo.get(repo) {
                    None => {
                        drifts.push(format!("{repo}: base 侧无此 repo (语料集漂移?), 人工核对"))
                    }
                    Some(base_agg) => {
                        let delta = if base_agg.comp_blk_pct == 0.0 {
                            0.0
                        } else {
                            (cur.comp_blk_pct - base_agg.comp_blk_pct) / base_agg.comp_blk_pct
                                * 100.0
                        };
                        if delta < -REGRESSION_MAX_PCT {
                            regressions.push(Regression {
                                repo: repo.clone(),
                                delta_blk_pct: delta,
                                cur_blk_pct: cur.comp_blk_pct,
                                base_blk_pct: base_agg.comp_blk_pct,
                            });
                        }
                    }
                }
            }
            let base_fail = !regressions.is_empty();
            CompressionCompare {
                mode: "双测对比",
                regressions,
                drift_notices: drifts,
                base_fail,
            }
        }
        None => CompressionCompare {
            mode: "单测 (未提供 base summary, 压缩率对比跳过 — CI 编排应提供 base 以启用退化哨兵)",
            regressions: Vec::new(),
            drift_notices: Vec::new(),
            base_fail: false,
        },
    };

    let verdict = if violations.is_empty() && !cmp.base_fail {
        "pass"
    } else {
        "fail"
    };
    Verdict {
        verdict,
        stats: serde_json::json!({
            "fn": t.fn_samples, "fp_rate_pct": t.fp_rate_pct,
            "zero_blocks": st.zero_block, "boundary_violations": st.boundary_violations,
            "samples": st.samples_total,
            "repos": st.repos_total,
            "comp_blk_pct": t.comp_blk_pct, "comp_sig_pct": t.comp_sig_pct,
            "per_rule_hits": &pr.per_rule_hits,
            "dead_rules": &dead_rules,
            // 双组聚合节 (T-P2b-2): gated = 断言面组值; exempt = 报告面镜像
            // (不进断言)。FN/FP率/零块按组分解; 边界断言全域 (引擎哨兵不豁免),
            // 报告面给出组内分解。
            "groups": {
                "gated": group_stats_json(
                    t,
                    st.zero_block,
                    st.boundary_violations.saturating_sub(st.exempt_boundary),
                ),
                "exempt": group_stats_json(
                    &pr.exempt_totals,
                    st.exempt_zero_block,
                    st.exempt_boundary,
                ),
            },
            "exempt": {
                "samples": pr.exempt_totals.runs,
                "repos": st.exempt_repos,
            },
        }),
        violations,
        compression_compare: cmp,
    }
}

impl Verdict {
    /// stderr 单行摘要 — 自解释 (issue #74): 阈值对照随行, pass 时验收线也一眼
    /// 可见, fail 时超标值与线同在。零阈值断言族用 `值/上限` (FN/零块/边界违例),
    /// 方向性断言用 `≤/≥` (FP 率上限 / 语料下限, 阈值字面量从常量派生);
    /// 压缩率无数值阈值 (双测对比专用) 与 规则零命中 (默认无断言) 只报值。
    /// 注: 值须经 as_f64/as_u64 取出后再 {:.2} 格式化 — serde_json::Value 的
    /// Display 忽略 precision 参数 (master 曾因此漏出全精度尾巴)。
    pub fn summary_line(&self) -> String {
        let num = |k: &str| self.stats[k].as_f64().unwrap_or(0.0);
        let cnt = |k: &str| self.stats[k].as_u64().unwrap_or(0);
        format!(
            "l0-gate: verdict={} FN={}/0 FP率={:.2}%≤{}% 零块={}/0 边界违例={}/0 \
             语料={}≥{} 仓={}≥{} 压缩 blk={:.2}% sig={:.2}% 规则零命中={}",
            self.verdict,
            cnt("fn"),
            num("fp_rate_pct"),
            FP_MAX_PCT,
            cnt("zero_blocks"),
            cnt("boundary_violations"),
            cnt("samples"),
            MIN_SAMPLES,
            cnt("repos"),
            MIN_REPOS,
            num("comp_blk_pct"),
            num("comp_sig_pct"),
            self.stats["dead_rules"].as_array().map_or(0, |a| a.len()),
        )
    }

    /// exempt 组状态单行 (T-P2b-2): None = 无豁免样本 (N=0 静默, 与
    /// attempt_missing_warning 同族 — 状态行只对真实豁免发出); 文本必含
    /// 计数 N / 仓数 M / 未入断言 说明 — 豁免余量是适配进度度量, 逐仓适配
    /// 后移出豁免 (sidecar 翻 false), 该行随之归零。
    pub fn exempt_line(&self) -> Option<String> {
        let n = self.stats["exempt"]["samples"].as_u64().unwrap_or(0);
        let m = self.stats["exempt"]["repos"].as_u64().unwrap_or(0);
        (n > 0).then(|| {
            format!(
                "l0: exempt: {n} 样本 ({m} 仓) 未入断言 \
                 (sidecar gate_exempt — 适配后移出, 豁免余量=适配进度)"
            )
        })
    }
}

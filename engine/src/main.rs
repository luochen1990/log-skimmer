//! CLI 薄壳 (spec §3): `log-skimmer skim <log> --rules <dir> [--json]`。
//! 职责边界: 参数解析 + 文件读取 + 调用纯核心 + 两种输出渲染; 无业务逻辑。
//!
//! 退出码契约 (SSOT 表 + 调用方行为见 README「退出码语义」): 0 正常完成 /
//! 2 运行期数据错误 (规则集非法、日志读取失败) / 3 零块失明 / 64 解析期用法错误
//! (BSD sysexits EX_USAGE; --help/--version 走 stdout 仍 0)。l0 门禁码 0/1/2
//! (解析成功后由 l0_cmd 运行期发出): 1=判定层违例 / 2=采集层语料或规则非法
//! (语料目录不存在/零配对样本/规则集非法, issue #44), 与解析期 64 正交。

use clap::builder::TypedValueParser as _;
use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

/// BSD sysexits(3) EX_USAGE: 命令用法错误 (解析期), 与运行期数据错误 exit 2 分流。
const EX_USAGE: i32 = 64;

/// 路径参数空串值解析器 (issue #73a): 原生 PathBufValueParser 对空串短路报空
/// invalid_value, 渲染层对空值统一渲染 "a value is required ... but none was
/// supplied" — 与真缺值 (clap verify_num_args 路径) 逐字节同文案, 与「用户
/// 提供了空串」的事实不符 (不可分诊)。inner 换不拒空串的 OsStringValueParser
/// (OsStr 直通, 非 UTF-8 路径行为不变) 再 try_map: 空串 → 专属文案 (风格对齐
/// MCP 侧 env 空串先例 "LOG_SKIMMER_RULES is set to an empty string"), 非空路径
/// 行为零变化; 真缺值发生在值抵达 parser 之前, 文案不受影响 (回归钉:
/// cli_exit.rs rules_missing_value_keeps_missing_copy)。三入口共用 (推广回归钉:
/// empty_path_values_get_dedicated_copy_across_subcommands)。
/// 文案单点: messages::empty_path_arg (issue #76)。
fn non_empty_arg(
    label: &'static str,
    hint: &'static str,
) -> impl clap::builder::TypedValueParser<Value = PathBuf> {
    clap::builder::OsStringValueParser::new().try_map(move |s: OsString| {
        if s.is_empty() {
            return Err(log_skimmer::messages::empty_path_arg(label, hint));
        }
        Ok(PathBuf::from(s))
    })
}

#[derive(Parser)]
#[command(
    name = "log-skimmer",
    version,
    about = "High-signal CI log extraction (POC)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Skim a log and print the extracted blocks
    Skim {
        /// Log file (exactly one; run per file for multiple logs; *.gz transparently decompressed); `-` reads stdin (Unix pipe convention)
        log_file: PathBuf,
        /// Rules directory (loads *.yaml recursively; an empty value gets a dedicated error — issue #73a)
        #[arg(long, value_parser = non_empty_arg("--rules", "must be a rules directory"))]
        rules: PathBuf,
        /// Output the JSON protocol (default: human-readable)
        #[arg(long)]
        json: bool,
        /// Disable output-layer repeat-collapse (default on: same-rule cascading blocks fold into the anchor block + a marker)
        #[arg(long)]
        no_collapse: bool,
        // 取值范围同界单源: engine lib 常量 CONTEXT_MAX (spec §1; issue #28) —
        // 与规则 context 字段及 MCP skim 参数共享, 漂移即编译错误;
        // 引用留在源码注释, 不进 help 文案 (用户可见文案去 spec 指针, issue #17)。
        // allow_negative_numbers (issue #52b): `-1` 这类负数字面量被送入值校验器
        // (invalid value ... not in 0..=10), 而非停在分词器的 `-- -1` 死路 tip;
        // 仅匹配负数形态, 其余 flag 仍按 flag 识别 (缺值报错不退化, 有 e2e 钉)。
        /// Override context globally to before=N/after=N (default: per-rule values; range 0-10, out of range is a usage error; values starting with - require the --context=N form)
        #[arg(long, allow_negative_numbers = true, value_parser = clap::value_parser!(u32).range(0..=log_skimmer::CONTEXT_MAX as i64))]
        context: Option<u32>,
    },
    /// L0 whole-corpus regression gate (collect→summarize→judge; exit 0=pass / 1=judge-layer fail / 2=invalid corpus or rules: corpus dir missing / zero paired samples / invalid ruleset; a small corpus 0<n<50 fails by design — use per_rule_hits for targeted validation; stream split: stdout=full JSON verdict report, stderr=one-line l0-gate summary)
    // 公开门禁工具 (issue #45): README 贡献教程 ③④ 与规则仓 CI 依赖 → 进公共
    // 帮助面; 可见性口径与 hygiene (#38) 互补 — 公开工具可见 / 内部发布链工具隐藏。
    L0 {
        /// Corpus root directory (recursive; a sample = a *.log or *.log.gz plus a matching *.log.json sidecar named like the log without the .gz suffix; one-sided orphans are warned on stderr and skipped)
        #[arg(long, value_parser = non_empty_arg("--corpus", "must be a corpus root directory"))]
        corpus: PathBuf,
        /// Rules directory (empty value gets the same dedicated error as skim --rules — issue #73a)
        #[arg(long, value_parser = non_empty_arg("--rules", "must be a rules directory"))]
        rules: PathBuf,
        /// Base-side summary JSON (dual-run compression comparison; omit for single-run mode)
        #[arg(long, value_parser = non_empty_arg("--base", "must be a base summary JSON file"))]
        base: Option<PathBuf>,
        /// Emit only the summary JSON (for measurement exchange; no judging)
        #[arg(long)]
        emit_summary: bool,
        // 正向验证: 存在语料零命中规则 (dead_rules) 时 fail (issue #19;
        // 默认关 — 存量规则集在公开语料上有天然零命中规则 (语料未覆盖的工具),
        // rules 仓 CI 与贡献者本地按需开; --emit-summary 优先于本开关, 不判定)。
        /// Positive validation: fail when a rule has zero corpus hits (issue #19; off by default; --emit-summary takes precedence — no judging)
        #[arg(long)]
        require_all_rules_hit: bool,
    },
    /// Credential-pattern hygiene check: scan a directory for credential shapes (mechanical gate for secret-scan/publish; 0=clean / 1=hits / 2=invalid list or directory)
    #[command(hide = true)] // 内部门禁工具, 不占公共帮助面 (issue #38; 与 l0 同款)
    Hygiene {
        /// Credential-pattern list file (one regex per line, # comments; SSOT data/secret-patterns.txt)
        #[arg(long, value_parser = non_empty_arg("--patterns", "must be a pattern list file"))]
        patterns: PathBuf,
        /// Directory to scan (git tree root, recursive)
        dir: PathBuf,
    },
}

fn main() {
    // 用法错误独立 exit 64 (issue #15): e.print() 按错误 Kind 自动选流 —
    // DisplayHelp/DisplayVersion 走 stdout 且 use_stderr()==false, 照常 exit 0;
    // 其余解析期错误 (未知参数/非法值/缺必选) 走 stderr, exit 64。
    let cli = Cli::try_parse().unwrap_or_else(|e| {
        let _ = e.print();
        std::process::exit(if e.use_stderr() { EX_USAGE } else { 0 });
    });
    match cli.cmd {
        Cmd::Skim {
            log_file,
            rules,
            json,
            no_collapse,
            context,
        } => skim_cmd(log_file, rules, json, no_collapse, context),
        Cmd::L0 {
            corpus,
            rules,
            base,
            emit_summary,
            require_all_rules_hit,
        } => l0_cmd(corpus, rules, base, emit_summary, require_all_rules_hit),
        Cmd::Hygiene { patterns, dir } => hygiene_cmd(patterns, dir),
    }
}

/// 运行期数据错误族单一出口 (退出码契约见文件头): stderr 报错 + exit 2。
/// 控制流辅助 (非文案), 文案由调用方经 messages 模块构造。
fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

fn hygiene_cmd(patterns: PathBuf, dir: PathBuf) {
    let pats = log_skimmer::hygiene::load_patterns(&patterns)
        .unwrap_or_else(|e| die(log_skimmer::messages::hygiene_patterns_load_failed(&e)));
    if !dir.is_dir() {
        die(log_skimmer::messages::hygiene_dir_not_found(&dir));
    }
    let hits = log_skimmer::hygiene::scan(&dir, &pats);
    if hits.is_empty() {
        println!("{}", log_skimmer::messages::hygiene_pass(pats.len(), &dir));
        return;
    }
    eprintln!("{}", log_skimmer::messages::hygiene_hits_header(hits.len()));
    for h in &hits {
        eprintln!(
            "  {}:{}: [{}] {}",
            h.path.display(),
            h.line,
            h.pattern,
            h.text
        );
    }
    std::process::exit(1);
}

fn l0_cmd(
    corpus: PathBuf,
    rules: PathBuf,
    base: Option<PathBuf>,
    emit_summary: bool,
    require_all_rules_hit: bool,
) {
    let collected = log_skimmer::l0::collect(&corpus, &rules)
        .unwrap_or_else(|e| die(log_skimmer::messages::l0_collect_failed(&e)));
    // 孤儿样本可见性 (issue #46): 配对缺口逐条 stderr 警告 (已跳过, 不计数) —
    // 自组语料的命名手误可见, 门禁不因配对缺口静默缩水; 是否升级硬错误留需求方。
    for o in &collected.orphans {
        let side = log_skimmer::messages::missing_side_label(&o.missing);
        eprintln!(
            "{}",
            log_skimmer::messages::l0_orphan_warning(&o.path, side)
        );
    }
    // sidecar 缺失 attempt 可见性 (P0-2): 聚合计数单行警告 — 统计按 attempt=1
    // 兜底不中断, 但缺口不得静默 (规范 §5 必带字段; 存量语料曾 57/57 缺失,
    // rerun-through 排除逻辑因此永不触发)。
    if let Some(w) = collected.attempt_missing_warning() {
        eprintln!("{w}");
    }
    // 零配对样本 = 采集层面失败: 语料非法 exit 2 (issue #44, 与规则侧对称) —
    // 拒绝在空语料上判定/出 summary (防空转假绿); 孤儿警告已先行给出线索。
    if collected.samples.is_empty() {
        die(log_skimmer::messages::l0_zero_paired_samples(&corpus));
    }
    let summary = log_skimmer::l0::summarize(&collected);
    if emit_summary {
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());
        return;
    }
    let base_summary = base.map(|p| {
        let raw = std::fs::read_to_string(&p)
            .unwrap_or_else(|e| die(log_skimmer::messages::l0_base_read_failed(&p, &e)));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| die(log_skimmer::messages::l0_base_parse_failed(&p, &e)))
    });
    let v = log_skimmer::l0::gate(&summary, base_summary.as_ref(), require_all_rules_hit);
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    eprintln!("{}", v.summary_line());
    // exempt 组状态单行 (T-P2b-2): verdict 行语义不变 (对 gated 组判定),
    // 豁免余量单列 — N=0 静默 (无豁免语料不增噪音)。
    if let Some(line) = v.exempt_line() {
        eprintln!("{line}");
    }
    if v.verdict != "pass" {
        std::process::exit(1);
    }
}

fn skim_cmd(
    log_file: PathBuf,
    rules: PathBuf,
    json: bool,
    no_collapse: bool,
    context: Option<u32>,
) {
    let rules_loaded = match log_skimmer::load_rules(&rules) {
        Ok(r) => match context {
            Some(n) => log_skimmer::override_context(&r, n),
            None => r,
        },
        Err(e) => die(e),
    };
    // #53 Unix 管道惯例: LOG_FILE 为 `-` 时读 stdin (32MB 上限仅 MCP 侧, CLI 文件
    // 输入无此限制 — stdin 与文件路径同口径); 读取失败报错标注来源是 stdin,
    // 不把 `-` 当文件名报「不存在」误导排障方向。
    let raw = if log_file == Path::new("-") {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .unwrap_or_else(|e| die(log_skimmer::messages::read_log_failed_stdin(&e)));
        buf
    } else {
        // 文件路径读取 (gz 透明解压单一入口, *.gz 与 plain 输出逐字节一致)
        log_skimmer::sample_io::read_log_sample(&log_file)
            .unwrap_or_else(|e| die(log_skimmer::messages::read_log_failed(&e)))
    };
    // 行归一: A0 (engine lib SSOT) — 剥首部 BOM (#78), 去 \r (CRLF), 保内容; 尾部空行不计。
    let lines = log_skimmer::normalize_lines(&raw);
    // 输入三分类 (issues #71/#78, spec §3): 0 行或纯空白 = 数据错误族 exit 2 硬拒绝,
    // 不进入零块判定 (与「空规则集硬拒绝」对称 — 宁可显式失败, 不静默通过);
    // 纯空白彻底离开零块路径, 排障话术 (检查日志源) 不再与回退 rg 建议同屏。
    // 拒斥文案 SSOT: engine input_reject_message (与 wasm ABI envelope 共用)。
    // 数据错误族单一出口 (exit 2), 文案逐字不变。
    if let Some(msg) = log_skimmer::input_reject_message(&lines) {
        die(msg);
    }
    let result = if no_collapse {
        log_skimmer::skim_with(
            &log_file.display().to_string(),
            &rules_loaded,
            &lines,
            &log_skimmer::CollapseParams::OFF,
        )
    } else {
        log_skimmer::skim(&log_file.display().to_string(), &rules_loaded, &lines)
    };
    let zero_blocks = result.blocks.is_empty();
    // 压缩率 (issue #17): 零块时无意义 → n/a (拒绝 100.0% 伪统计);
    // input_lines 恒 > 0 (空/纯空白已在三分类前置拦截, issues #71/#78)。
    let pct = if zero_blocks {
        "n/a".to_string()
    } else {
        format!(
            "{:.1}%",
            100.0 - 100.0 * result.output_lines as f64 / result.input_lines as f64
        )
    };
    // 折叠审计尾注 (spec §3.R2): 零折叠时不追加 — 零行为变更保证 (文案单点 messages)。
    let fold_note = if result.stats.collapse.folded_blocks > 0 {
        log_skimmer::messages::fold_note(
            result.stats.collapse.folded_blocks,
            result.stats.collapse.omitted_lines,
        )
    } else {
        String::new()
    };
    let stats_line = log_skimmer::messages::stats_line(
        result.input_lines,
        result.output_lines,
        &pct,
        result.stats.noise_lines_removed,
        result.stats.boundary_violations.len(),
        &fold_note,
    );
    if json {
        // --json 流分离: stdout 纯 JSON (程序化消费), 诊断走 stderr。
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        eprintln!("{stats_line}");
    } else {
        // 人类可读输出整体走 stdout (issue #10): 块内容与统计行同为「输出成果」,
        // `> out.txt` 重定向完整保留; 错误/警告 (零块警示) 才走 stderr。
        for (i, b) in result.blocks.iter().enumerate() {
            // #78b: 首块前无前导空行 (输出顶格开始); 块间空行保留作块分隔。
            if i > 0 {
                println!();
            }
            let rep = b.representative();
            println!(
                "--- [L{}-L{}] [{}/{}] {}",
                b.start_line,
                b.end_line,
                rep.intent,
                rep.category,
                b.rules.join(",")
            );
            for l in b.start_line..=b.end_line {
                if !result.noise_lines.contains(&l) {
                    println!("{}", lines[l - 1]);
                }
            }
            // 折叠行 (spec §3.R2): 锚块文本之后, 单行, 不计入 output_lines。
            if let Some(m) = &b.collapse {
                println!("{}", m.render_line());
            }
        }
        println!();
        println!("{stats_line}");
    }
    // 零块警示 (无条件): 「任何真实 CI 日志 skim 非零」是规则集的结构承诺
    // (失败侧 annotation/runner 安全网 + 成功侧 runner 版本 info 规则) — 零块 = 规则集
    // 对该日志失明, 与 job 成败无关, 调用方应无条件回退 rg 基线。
    // #71a: 纯空白输入已在上方三分类处 exit 2 前置拦截, 不达此处 — 警示恒为
    // 失明导向文案, 无需按输入形态分流 (#51 分流形态随纯空白归 exit 2 退役)。
    if zero_blocks {
        eprintln!("{}", log_skimmer::messages::zero_blocks_warning());
        std::process::exit(3);
    }
}

#[cfg(test)]
mod tests {
    //! `--context` 参数契约 (issue #11): 取值 0-CONTEXT_MAX, 与 MCP skim 的
    //! `context` schema 同界 — 同界经 engine lib 单源常量 CONTEXT_MAX (issue #28),
    //! 边界断言值均从常量派生 (改常量即测试边界同步变); 越界在解析期即用法错误,
    //! 不进入执行路径。

    use super::*;

    /// 解析 skim 子命令并取 `--context` 值 (Err = clap 用法错误)。
    fn skim_context(extra: &[&str]) -> Result<Option<u32>, clap::Error> {
        let mut argv: Vec<&str> = vec!["log-skimmer", "skim", "x.log", "--rules", "r"];
        argv.extend_from_slice(extra);
        Cli::try_parse_from(argv).map(|cli| match cli.cmd {
            Cmd::Skim { context, .. } => context,
            _ => unreachable!("skim 子命令"),
        })
    }

    #[test]
    fn context_out_of_range_is_usage_error() {
        // -1 两种拼写钉两条拒绝路径: 空格形态止于 clap 分词器 (unexpected
        // argument), 等号形态才走 range 校验器 (invalid value ... not in 上界内)。
        let over = (log_skimmer::CONTEXT_MAX + 1).to_string();
        for bad in [
            ["--context", over.as_str()].as_slice(),
            ["--context", "999"].as_slice(),
            ["--context", "-1"].as_slice(),
            ["--context=-1"].as_slice(),
        ] {
            assert!(skim_context(bad).is_err(), "{bad:?} 须被拒绝");
        }
    }

    #[test]
    fn context_boundaries_and_default_pass() {
        assert_eq!(skim_context(&["--context", "0"]).unwrap(), Some(0));
        assert_eq!(
            skim_context(&["--context", &log_skimmer::CONTEXT_MAX.to_string()]).unwrap(),
            Some(log_skimmer::CONTEXT_MAX)
        );
        assert_eq!(skim_context(&[]).unwrap(), None);
    }
}

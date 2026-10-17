//! CLI 薄壳 (spec §3): `log-skimmer skim <log> --rules <dir> [--json]`。
//! 职责边界: 参数解析 + 文件读取 + 调用纯核心 + 两种输出渲染; 无业务逻辑。
//!
//! 退出码契约 (SSOT 表 + 调用方行为见 README「退出码语义」): 0 正常完成 /
//! 2 运行期数据错误 (规则集非法、日志读取失败) / 3 零块失明 / 64 解析期用法错误
//! (BSD sysexits EX_USAGE; --help/--version 走 stdout 仍 0)。l0 判定码 0/1/2 是
//! 内部 CI 契约 (解析成功后由 l0_cmd 运行期发出), 与解析期 64 正交。

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// BSD sysexits(3) EX_USAGE: 命令用法错误 (解析期), 与运行期数据错误 exit 2 分流。
const EX_USAGE: i32 = 64;

#[derive(Parser)]
#[command(name = "log-skimmer", version, about = "CI 日志高信噪比提取 (POC)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 扫描日志, 输出提取块
    Skim {
        log_file: PathBuf,
        /// 规则目录 (递归加载 *.yaml)
        #[arg(long)]
        rules: PathBuf,
        /// 输出 JSON 协议 (默认人类可读)
        #[arg(long)]
        json: bool,
        // 取值范围 SSOT: 与规则 context 字段及 MCP skim 参数同界 (spec §1);
        // 引用留在源码注释, 不进 help 文案 (用户可见文案去 spec 指针, issue #17)。
        /// 全局覆盖 context 为 before=N/after=N (默认按规则自带; 取值 0-10, 越界为用法错误)
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=10))]
        context: Option<u32>,
    },
    /// L0 全语料回归门禁 (采集→汇总→判定; 退出码 0=pass / 1=fail / 2=语料或规则非法)
    #[command(hide = true)] // 内部 CI 工具, 不占公共帮助面 (issue #16)
    L0 {
        /// 语料根目录 (含 *.log + *.log.json 递归)
        #[arg(long)]
        corpus: PathBuf,
        /// 规则目录
        #[arg(long)]
        rules: PathBuf,
        /// base 侧 summary JSON (双测压缩率对比; 缺省单测模式)
        #[arg(long)]
        base: Option<PathBuf>,
        /// 仅输出 summary JSON (实测交换用, 不判定)
        #[arg(long)]
        emit_summary: bool,
    },
    /// 发布面卫生检查: 扫描目录内网模式 (publish 必经机械防线; 0=干净 / 1=命中 / 2=清单或目录非法)
    Hygiene {
        /// 内网模式清单文件 (一行一模式, # 注释; 内网专用件不随发布面, SSOT 见 AGENTS.md 公开镜像纪律)
        #[arg(long)]
        patterns: PathBuf,
        /// 待扫描目录 (发布树根, 递归)
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
            context,
        } => skim_cmd(log_file, rules, json, context),
        Cmd::L0 {
            corpus,
            rules,
            base,
            emit_summary,
        } => l0_cmd(corpus, rules, base, emit_summary),
        Cmd::Hygiene { patterns, dir } => hygiene_cmd(patterns, dir),
    }
}

fn hygiene_cmd(patterns: PathBuf, dir: PathBuf) {
    let pats = log_skimmer::hygiene::load_patterns(&patterns).unwrap_or_else(|e| {
        eprintln!("hygiene: 模式清单装载失败 — {e}");
        std::process::exit(2);
    });
    if !dir.is_dir() {
        eprintln!("hygiene: 目录不存在 {}", dir.display());
        std::process::exit(2);
    }
    let hits = log_skimmer::hygiene::scan(&dir, &pats);
    if hits.is_empty() {
        println!(
            "hygiene: 通过 — {} 项内网模式零命中 ({})",
            pats.len(),
            dir.display()
        );
        return;
    }
    eprintln!(
        "hygiene: 内网模式命中 {} 处 — 禁止发布 (issue #18 机械防线; 修复: 排除清单 justfile publish-stage / 源头改写):",
        hits.len()
    );
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

fn l0_cmd(corpus: PathBuf, rules: PathBuf, base: Option<PathBuf>, emit_summary: bool) {
    let samples = log_skimmer::l0::collect(&corpus, &rules).unwrap_or_else(|e| {
        eprintln!("l0 采集失败: {e}");
        std::process::exit(2);
    });
    let summary = log_skimmer::l0::summarize(&samples);
    if emit_summary {
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());
        return;
    }
    let base_summary = base.map(|p| {
        let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| {
            eprintln!("base summary 读取失败 {}: {e}", p.display());
            std::process::exit(2);
        });
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            eprintln!("base summary 解析失败 {}: {e}", p.display());
            std::process::exit(2);
        })
    });
    let v = log_skimmer::l0::gate(&summary, base_summary.as_ref());
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    eprintln!(
        "l0-gate: verdict={} FN={} FP率={:.2}% 零块={} 边界违例={} 压缩 blk={:.2}% sig={:.2}%",
        v.verdict,
        v.stats["fn"],
        v.stats["fp_rate_pct"],
        v.stats["zero_blocks"],
        v.stats["boundary_violations"],
        v.stats["comp_blk_pct"],
        v.stats["comp_sig_pct"]
    );
    if v.verdict != "pass" {
        std::process::exit(1);
    }
}

fn skim_cmd(log_file: PathBuf, rules: PathBuf, json: bool, context: Option<u32>) {
    let rules_loaded = match log_skimmer::load_rules(&rules) {
        Ok(r) => match context {
            Some(n) => log_skimmer::override_context(&r, n),
            None => r,
        },
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let raw = std::fs::read_to_string(&log_file).unwrap_or_else(|e| {
        eprintln!("读取日志失败 {}: {e}", log_file.display());
        std::process::exit(2);
    });
    // 行归一: 去 \r (CRLF), 保内容; 尾部空行不计 (A0, engine lib SSOT)。
    let lines = log_skimmer::normalize_lines(&raw);
    // 空日志硬拒绝 (issue #13): 0 行 = 疑似下载截断的数据错误, 不进入零块判定
    // (与「空规则集硬拒绝」哲学对称 — 宁可显式失败, 不静默通过)。
    if lines.is_empty() {
        eprintln!("日志为空 (0 行): 疑似下载截断, 拒绝静默通过");
        std::process::exit(2);
    }
    let result = log_skimmer::skim(&log_file.display().to_string(), &rules_loaded, &lines);
    let zero_blocks = result.blocks.is_empty();
    // 压缩率 (issue #17): 零块时无意义 → n/a (拒绝 100.0% 伪统计);
    // input_lines 恒 > 0 (空日志已前置拦截)。
    let pct = if zero_blocks {
        "n/a".to_string()
    } else {
        format!(
            "{:.1}%",
            100.0 - 100.0 * result.output_lines as f64 / result.input_lines as f64
        )
    };
    let stats_line = format!(
        "input {} lines -> output {} lines ({pct} compression), noise removed {}, boundary violations {}",
        result.input_lines,
        result.output_lines,
        result.stats.noise_lines_removed,
        result.stats.boundary_violations.len()
    );
    if json {
        // --json 流分离: stdout 纯 JSON (程序化消费), 诊断走 stderr。
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        eprintln!("{stats_line}");
    } else {
        // 人类可读输出整体走 stdout (issue #10): 块内容与统计行同为「输出成果」,
        // `> out.txt` 重定向完整保留; 错误/警告 (零块警示) 才走 stderr。
        for b in &result.blocks {
            println!();
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
        }
        println!();
        println!("{stats_line}");
    }
    // 零块警示 (无条件): 「任何真实 CI 日志 skim 非零」是规则集的结构承诺
    // (失败侧 annotation/runner 安全网 + 成功侧 runner 版本 info 规则) — 零块 = 规则集
    // 对该日志失明, 与 job 成败无关, 调用方应无条件回退 rg 基线。
    if zero_blocks {
        eprintln!(
            "⚠ 零提取 — 规则集对该日志失明; 建议回退 rg error|warn && tail 基线; 新工具日志可贡献规则: https://github.com/luochen1990/log-skimmer-rules"
        );
        std::process::exit(3);
    }
}

#[cfg(test)]
mod tests {
    //! `--context` 参数契约 (issue #11): 取值 0-10, 与 MCP skim 的 `context`
    //! schema (server/src/mcp.rs) 同界; 越界在解析期即用法错误, 不进入执行路径。

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
        // argument), 等号形态才走 range 校验器 (invalid value ... not in 0..=10)。
        for bad in [
            ["--context", "11"].as_slice(),
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
        assert_eq!(skim_context(&["--context", "10"]).unwrap(), Some(10));
        assert_eq!(skim_context(&[]).unwrap(), None);
    }
}

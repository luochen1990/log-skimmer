//! CLI 薄壳 (spec §3): `log-skimmer skim <log> --rules <dir> [--json]`。
//! 职责边界: 参数解析 + 文件读取 + 调用纯核心 + 两种输出渲染; 无业务逻辑。

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "log-skimmer", about = "CI 日志高信噪比提取 (POC)")]
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
        /// 输出 spec §3 JSON 协议 (默认人类可读)
        #[arg(long)]
        json: bool,
        /// 全局覆盖 context 为 before=N/after=N (默认按规则自带)
        #[arg(long)]
        context: Option<u32>,
    },
    /// L0 全语料回归门禁 (采集→汇总→判定; 退出码 0=pass / 1=fail / 2=语料或规则非法)
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
    let cli = Cli::parse();
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
    let result = log_skimmer::skim(&log_file.display().to_string(), &rules_loaded, &lines);
    let compression = if result.input_lines > 0 {
        100.0 - 100.0 * result.output_lines as f64 / result.input_lines as f64
    } else {
        0.0
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    } else {
        for b in &result.blocks {
            eprintln!();
            println!(
                "--- [L{}-L{}] {}",
                b.start_line,
                b.end_line,
                b.rules.join(",")
            );
            for l in b.start_line..=b.end_line {
                if !result.noise_lines.contains(&l) {
                    println!("{}", lines[l - 1]);
                }
            }
        }
    }
    eprintln!(
        "input {} lines -> output {} lines ({:.1}% compression), noise removed {}, boundary violations {}",
        result.input_lines, result.output_lines, compression,
        result.stats.noise_lines_removed, result.stats.boundary_violations.len()
    );
    // 零块警示 (spec §3, 无条件): 「任何真实 CI 日志 skim 非零」是规则集的结构承诺
    // (失败侧 annotation/runner 安全网 + 成功侧 runner 版本 info 规则) — 零块 = 规则集
    // 对该日志失明, 与 job 成败无关, 调用方应无条件回退 rg 基线。
    if result.blocks.is_empty() && result.input_lines > 0 {
        eprintln!(
            "⚠ 零提取 — 规则集对该日志失明 (spec §0 召回优先); 建议回退 rg error|warn && tail 基线"
        );
        std::process::exit(3);
    }
}

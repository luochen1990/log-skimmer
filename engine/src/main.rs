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
}

fn main() {
    let cli = Cli::parse();
    let Cmd::Skim {
        log_file,
        rules,
        json,
        context,
    } = cli.cmd;
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

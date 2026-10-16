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
    },
}

fn main() {
    let cli = Cli::parse();
    let Cmd::Skim {
        log_file,
        rules,
        json,
    } = cli.cmd;
    let rules_loaded = match log_skimmer::load_rules(&rules) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let raw = std::fs::read_to_string(&log_file).unwrap_or_else(|e| {
        eprintln!("读取日志失败 {}: {e}", log_file.display());
        std::process::exit(2);
    });
    // 行归一: 去 \r (CRLF), 保内容; 尾部空行不计。
    let lines: Vec<String> = raw
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
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
}

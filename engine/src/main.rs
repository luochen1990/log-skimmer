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
/// MCP 侧 env 空串先例 "LOG_SKIMMER_RULES 为空串"), 非空路径行为零变化; 真缺值
/// 发生在值抵达 parser 之前, 文案不受影响 (回归钉: cli_exit.rs
/// rules_missing_value_keeps_missing_copy)。三入口共用 (推广回归钉:
/// empty_path_values_get_dedicated_copy_across_subcommands)。
fn non_empty_arg(
    label: &'static str,
    hint: &'static str,
) -> impl clap::builder::TypedValueParser<Value = PathBuf> {
    clap::builder::OsStringValueParser::new().try_map(move |s: OsString| {
        if s.is_empty() {
            return Err(format!("{label} 不能为空串 ({hint})"));
        }
        Ok(PathBuf::from(s))
    })
}

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
        /// 日志文件 (只接受一个; 多文件须分别调用), `-` 表示 stdin (Unix 管道惯例)
        log_file: PathBuf,
        /// 规则目录 (递归加载 *.yaml; 空串有专属报错, 见 non_empty_arg — issue #73a)
        #[arg(long, value_parser = non_empty_arg("--rules", "须为规则集目录"))]
        rules: PathBuf,
        /// 输出 JSON 协议 (默认人类可读)
        #[arg(long)]
        json: bool,
        // 取值范围同界单源: engine lib 常量 CONTEXT_MAX (spec §1; issue #28) —
        // 与规则 context 字段及 MCP skim 参数共享, 漂移即编译错误;
        // 引用留在源码注释, 不进 help 文案 (用户可见文案去 spec 指针, issue #17)。
        // allow_negative_numbers (issue #52b): `-1` 这类负数字面量被送入值校验器
        // (invalid value ... not in 0..=10), 而非停在分词器的 `-- -1` 死路 tip;
        // 仅匹配负数形态, 其余 flag 仍按 flag 识别 (缺值报错不退化, 有 e2e 钉)。
        /// 全局覆盖 context 为 before=N/after=N (默认按规则自带; 取值 0-10, 越界为用法错误; 以 - 开头的值请用 --context=N 形式)
        #[arg(long, allow_negative_numbers = true, value_parser = clap::value_parser!(u32).range(0..=log_skimmer::CONTEXT_MAX as i64))]
        context: Option<u32>,
    },
    /// L0 全语料回归门禁 (采集→汇总→判定; 退出码 0=pass / 1=fail / 2=语料或规则非法:
    /// 语料目录不存在/零配对样本/规则集非法; 0<n<50 小语料属判定 fail — 定向验证以 per_rule_hits 为信号;
    /// 流分离: stdout=完整 JSON 判定报告 (可重定向存档), stderr=l0-gate 单行摘要)
    // 公开门禁工具 (issue #45): README 贡献教程 ③④ 与规则仓 CI 依赖 → 进公共
    // 帮助面; 可见性口径与 hygiene (#38) 互补 — 公开工具可见 / 内部发布链工具隐藏。
    L0 {
        /// 语料根目录 (递归; 样本 = *.log + 同名 *.log.json 配对, 单侧缺失的孤儿样本 stderr 警告并跳过)
        #[arg(long, value_parser = non_empty_arg("--corpus", "须为语料根目录"))]
        corpus: PathBuf,
        /// 规则目录 (空串专属报错同 skim --rules — issue #73a 推广)
        #[arg(long, value_parser = non_empty_arg("--rules", "须为规则集目录"))]
        rules: PathBuf,
        /// base 侧 summary JSON (双测压缩率对比; 缺省单测模式)
        #[arg(long, value_parser = non_empty_arg("--base", "须为 base summary JSON 文件"))]
        base: Option<PathBuf>,
        /// 仅输出 summary JSON (实测交换用, 不判定)
        #[arg(long)]
        emit_summary: bool,
        /// 正向验证: 存在语料零命中规则 (dead_rules) 时 fail (issue #19;
        /// 默认关 — 存量规则集在公开语料上有天然零命中规则 (语料未覆盖的工具),
        /// rules 仓 CI 与贡献者本地按需开; --emit-summary 优先于本开关, 不判定)
        #[arg(long)]
        require_all_rules_hit: bool,
    },
    /// 发布面卫生检查: 扫描目录内网模式 (publish 必经机械防线; 0=干净 / 1=命中 / 2=清单或目录非法)
    #[command(hide = true)] // 内部发布链工具, 不占公共帮助面 (issue #38; 与 l0 同款)
    Hygiene {
        /// 内网模式清单文件 (一行一模式, # 注释; 内网专用件不随发布面, 排除口径 SSOT 见 justfile publish-stage)
        #[arg(long, value_parser = non_empty_arg("--patterns", "须为模式清单文件"))]
        patterns: PathBuf,
        /// 待扫描目录 (发布树根, 递归)
        dir: PathBuf,
    },
    /// 发布面 tag 卫生检查: 远端 refs/tags 每 tag 须指向 orphan 快照 commit 且树无内网专用件
    /// (publish 必经机械防线, 2026-10-20 tag 残留事故补口; 0=合规 / 1=违规 / 2=远端或清单非法)
    #[command(hide = true)] // 内部发布链工具, 不占公共帮助面 (与 hygiene/l0 同款)
    TagGuard {
        /// 发布面远端 (https://... / ssh://... / 本地 bare 仓路径)
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        remote: String,
        /// 排除清单文件 (一行一路径前缀, # 注释; 与 publish-stage 发布树构造 rm 共源 SSOT)
        #[arg(long, value_parser = non_empty_arg("--excludes", "须为排除清单文件"))]
        excludes: PathBuf,
    },
    /// 排除清单装载校验 + 条目逐行输出 (publish-stage 的 rm 消费入口;
    /// 校验 SSOT 在 engine 装载器 — bash 只消费已校验条目, 不平行实现校验)
    #[command(hide = true)] // 内部发布链工具, 不占公共帮助面 (与 hygiene/l0 同款)
    Excludes {
        /// 排除清单文件
        #[arg(value_parser = non_empty_arg("排除清单路径", "位置参数"))]
        file: PathBuf,
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
            require_all_rules_hit,
        } => l0_cmd(corpus, rules, base, emit_summary, require_all_rules_hit),
        Cmd::Hygiene { patterns, dir } => hygiene_cmd(patterns, dir),
        Cmd::TagGuard { remote, excludes } => tag_guard_cmd(remote, excludes),
        Cmd::Excludes { file } => excludes_cmd(file),
    }
}

fn excludes_cmd(file: PathBuf) {
    let entries = log_skimmer::tag_guard::load_excludes(&file).unwrap_or_else(|e| {
        eprintln!("excludes: 排除清单装载失败 — {e}");
        std::process::exit(2);
    });
    for e in &entries {
        println!("{e}");
    }
}

fn tag_guard_cmd(remote: String, excludes: PathBuf) {
    let ex = log_skimmer::tag_guard::load_excludes(&excludes).unwrap_or_else(|e| {
        eprintln!("tag-guard: 排除清单装载失败 — {e}");
        std::process::exit(2);
    });
    let audits = log_skimmer::tag_guard::guard(&remote, &ex).unwrap_or_else(|e| {
        eprintln!("tag-guard: 采集失败 — {e}");
        std::process::exit(2);
    });
    let violations: Vec<_> = audits.iter().filter(|a| !a.ok()).collect();
    if violations.is_empty() {
        println!(
            "tag-guard: 通过 — 远端 tag 面 {} tags 全部为合法快照 (orphan + 无内网专用件): {remote}",
            audits.len()
        );
        return;
    }
    eprintln!(
        "tag-guard: 违规 {} / {} tags — 发布面 tag 须指向 orphan 快照 commit, 排除件连同历史不残留 (issue #18; 2026-10-20 tag 残留事故补口):",
        violations.len(),
        audits.len()
    );
    for a in &violations {
        if let Some(kind) = &a.malformed {
            eprintln!(
                "  refs/tags/{}: 指向非 commit 对象 ({kind}) — 畸形 tag 面",
                a.tag
            );
        }
        if a.parents > 0 {
            eprintln!(
                "  refs/tags/{}: 携带 {} parent (非 orphan) — 历史链经 tag 可达, 内网专用件可能随历史泄露",
                a.tag, a.parents
            );
        }
        for h in &a.hits {
            eprintln!("  refs/tags/{}: 树含内网专用件 {h}", a.tag);
        }
    }
    eprintln!("修复: git push <remote> :refs/tags/<tag> 删除, 或将 tag 迁移至快照 commit");
    std::process::exit(1);
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

fn l0_cmd(
    corpus: PathBuf,
    rules: PathBuf,
    base: Option<PathBuf>,
    emit_summary: bool,
    require_all_rules_hit: bool,
) {
    let collected = log_skimmer::l0::collect(&corpus, &rules).unwrap_or_else(|e| {
        eprintln!("l0 采集失败: {e}");
        std::process::exit(2);
    });
    // 孤儿样本可见性 (issue #46): 配对缺口逐条 stderr 警告 (已跳过, 不计数) —
    // 自组语料的命名手误可见, 门禁不因配对缺口静默缩水; 是否升级硬错误留需求方。
    for o in &collected.orphans {
        let side = match o.missing {
            log_skimmer::l0::MissingSide::Sidecar => "sidecar (*.log.json)",
            log_skimmer::l0::MissingSide::Log => "日志 (*.log)",
        };
        eprintln!("l0: 警告: 孤儿样本 {} 缺 {side}, 已跳过 (不计数)", o.path);
    }
    // 零配对样本 = 采集层面失败: 语料非法 exit 2 (issue #44, 与规则侧对称) —
    // 拒绝在空语料上判定/出 summary (防空转假绿); 孤儿警告已先行给出线索。
    if collected.samples.is_empty() {
        eprintln!(
            "l0: 语料零配对样本 ({}) — 语料非法, 拒绝判定 (检查目录内容/获取链路)",
            corpus.display()
        );
        std::process::exit(2);
    }
    let summary = log_skimmer::l0::summarize(&collected);
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
    let v = log_skimmer::l0::gate(&summary, base_summary.as_ref(), require_all_rules_hit);
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    eprintln!("{}", v.summary_line());
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
    // #53 Unix 管道惯例: LOG_FILE 为 `-` 时读 stdin (32MB 上限仅 MCP 侧, CLI 文件
    // 输入无此限制 — stdin 与文件路径同口径); 读取失败报错标注来源是 stdin,
    // 不把 `-` 当文件名报「不存在」误导排障方向。
    let raw = if log_file == Path::new("-") {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .unwrap_or_else(|e| {
                eprintln!("读取日志失败 (stdin): {e}");
                std::process::exit(2);
            });
        buf
    } else {
        std::fs::read_to_string(&log_file).unwrap_or_else(|e| {
            eprintln!("读取日志失败 {}: {e}", log_file.display());
            std::process::exit(2);
        })
    };
    // 行归一: A0 (engine lib SSOT) — 剥首部 BOM (#78), 去 \r (CRLF), 保内容; 尾部空行不计。
    let lines = log_skimmer::normalize_lines(&raw);
    // 输入三分类 (issues #71/#78, spec §3): 0 行或纯空白 = 数据错误族 exit 2 硬拒绝,
    // 不进入零块判定 (与「空规则集硬拒绝」对称 — 宁可显式失败, 不静默通过);
    // 纯空白彻底离开零块路径, 排障话术 (检查日志源) 不再与回退 rg 建议同屏。
    let reject = match log_skimmer::classify_input(&lines) {
        log_skimmer::InputClass::Empty => {
            Some("日志为空 (0 行): 疑似下载截断, 拒绝静默通过".to_string())
        }
        log_skimmer::InputClass::WhitespaceOnly => Some(format!(
            "日志为纯空白 ({} 行均为空白行): 疑似下载截断或空跑, 请检查日志源, 拒绝静默通过",
            lines.len()
        )),
        log_skimmer::InputClass::Content => None,
    };
    // 数据错误族单一出口 (exit 2), 文案逐字不变。
    if let Some(msg) = reject {
        eprintln!("{msg}");
        std::process::exit(2);
    }
    let result = log_skimmer::skim(&log_file.display().to_string(), &rules_loaded, &lines);
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
        eprintln!("⚠ 零提取 — 规则集对该日志失明; 建议回退 rg error|warn && tail 基线; 新工具日志可贡献规则: https://github.com/luochen1990/log-skimmer-rules");
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

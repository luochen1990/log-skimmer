//! 日志样本读取单一入口 (gz 透明解压) — spec §5 压缩形态。
//!
//! 职责边界: 文件→文本 的唯一通道, CLI skim 单文件输入 (main.rs) 与 l0 语料
//! 遍历 (l0.rs) 共用, 禁止消费点自写解压。读取形态由路径扩展名驱动:
//! `*.gz` → gzip 解压, 其余原样读取 (非魔数探测 — 配对与形态约定见 l0.rs)。
//! 契约: 读取/解压/UTF-8 解码失败 → Err 恒为 "{路径}: {原因}" 形态 (动词前缀
//! 留给调用方语境渲染: CLI "读取日志失败" / l0 "日志读取失败"), fail-fast
//! 不静默跳过 — 与装载失败哲学一致 (截断/非法样本宁可显式失败, 不取残卷)。

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// 读取日志样本: `*.gz` 走 gzip 解压 (多成员流全量解码 — 单成员解码器对
/// cat 拼接流会静默丢尾, 违反不静默契约), 其余原样读取。
pub fn read_log_sample(path: &Path) -> Result<String, String> {
    let read_err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut buf = String::new();
    if path.extension().is_some_and(|x| x == "gz") {
        let f = File::open(path).map_err(read_err)?;
        flate2::read::MultiGzDecoder::new(f)
            .read_to_string(&mut buf)
            .map_err(read_err)?;
    } else {
        File::open(path)
            .map_err(read_err)?
            .read_to_string(&mut buf)
            .map_err(read_err)?;
    }
    Ok(buf)
}

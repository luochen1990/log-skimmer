//! 日志样本读取 (gz 透明) 契约测试: `sample_io::read_log_sample` 是 文件→文本
//! 的单一入口 (CLI skim 单文件输入 与 l0 语料遍历共用, 禁止消费点自写解压)。
//! 契约: 扩展名驱动 (*.gz 解压, 其余原样 — 非魔数探测); 解压/解码失败 → Err
//! 恒含文件路径 (fail-fast 不静默跳过, 与装载失败哲学一致)。
//! gz 测试数据由测试用 flate2 现场写出, 不入库二进制 fixture。

use log_skimmer::sample_io::read_log_sample;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 临时目录助手 (仓内惯例同 cli_contract: ls-<套件>-<名>, 复跑先清)。
fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ls-sample-io-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 现场写出单成员 gzip 文件。
fn write_gz(path: &Path, content: &str) {
    let mut w = flate2::write::GzEncoder::new(
        std::fs::File::create(path).unwrap(),
        flate2::Compression::default(),
    );
    w.write_all(content.as_bytes()).unwrap();
    w.finish().unwrap();
}

/// gz 解压还原; 非 .gz 路径原样读取 (扩展名驱动 — 内容恰似 gzip 魔数也不探测)。
#[test]
fn gz_decodes_and_plain_passes_through() {
    let d = tmp_dir("roundtrip");
    let body = "error: boom\nRunner Image: ubuntu\n";
    let gz = d.join("a.log.gz");
    write_gz(&gz, body);
    assert_eq!(read_log_sample(&gz).unwrap(), body);
    let plain = d.join("a.log");
    std::fs::write(&plain, body).unwrap();
    assert_eq!(read_log_sample(&plain).unwrap(), body);
}

/// 多成员 gzip 流 (cat a.gz b.gz 形态) 全量解码 — 单成员解码器会静默丢尾,
/// 违反「不静默跳过」; 语料仓工具链换代不引入静默截断。
#[test]
fn multi_member_gz_decodes_fully() {
    let d = tmp_dir("multi");
    let member = |s: &str| {
        let mut w = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        w.write_all(s.as_bytes()).unwrap();
        w.finish().unwrap()
    };
    let gz = d.join("m.log.gz");
    std::fs::write(&gz, [member("part one\n"), member("part two\n")].concat()).unwrap();
    assert_eq!(read_log_sample(&gz).unwrap(), "part one\npart two\n");
}

/// 非法 gz (明文误命名 .gz): Err 带文件路径, 不静默按空/垃圾内容通过。
#[test]
fn invalid_gz_fails_fast_with_path() {
    let d = tmp_dir("invalid");
    let gz = d.join("bad.log.gz");
    std::fs::write(&gz, "this is not a gzip stream").unwrap();
    let err = read_log_sample(&gz).unwrap_err();
    assert!(err.contains("bad.log.gz"), "错误须带文件路径: {err}");
}

/// 截断 gz (下载中断形态): Err 带文件路径 — 宁可显式失败, 不静默取残卷。
#[test]
fn truncated_gz_fails_fast_with_path() {
    let d = tmp_dir("truncated");
    let gz = d.join("cut.log.gz");
    write_gz(&gz, &"error: filler line\n".repeat(200));
    let whole = std::fs::read(&gz).unwrap();
    std::fs::write(&gz, &whole[..whole.len() / 2]).unwrap();
    let err = read_log_sample(&gz).unwrap_err();
    assert!(err.contains("cut.log.gz"), "截断不静默, 错误带路径: {err}");
}

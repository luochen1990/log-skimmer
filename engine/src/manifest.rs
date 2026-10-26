//! 规则集 manifest: 集级版本协商 (spec §1.M, issue #59)。
//!
//! 职责边界: 解析规则集的 `manifest.yaml` 并做兼容性判定 — `format_version`
//! (规则格式版本上界) 与 `min_engine_version` (引擎版本下限)。判定违例 =
//! 规则集非法 (整体拒载, exit 2 族); Err 文案自解释 (哪个校验失败 + 建议动作
//! — 消费方一读即知该升级哪一侧)。版本比较是朴素 major.minor.patch 数字
//! 三元组比较, 不引 semver crate (依赖纪律: engine 限 serde/serde_yaml/regex/clap)。

use serde::Deserialize;
use std::path::Path;

/// 本引擎支持的规则格式版本 (spec §1.M: 当前 2)。
/// v2 = scope/repo 弃用窗口 (spec §1.S): v2 引擎同时接受带/不带该两字段的规则
/// 文件 (值忽略); 声明 format_version: 2 的规则集在 v1 引擎上拒载。
/// 规则格式不兼容演进时 bump; 兼容演进 (已知字段内) 不 bump。
pub const FORMAT_VERSION: u32 = 2;

/// manifest 保留文件名 (规则集根目录, 集级非每规则; load_rules 跳过其规则解析)。
pub const MANIFEST_FILENAME: &str = "manifest.yaml";

/// 规则集 manifest (spec §1.M)。deny_unknown_fields: manifest 新字段在旧引擎上
/// 显式拒载 (静默偏移比拒载更危险)。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesetManifest {
    /// 规则格式版本 (必填, 整数)。
    pub format_version: u32,
    /// 引擎版本下限 (选填, major.minor.patch 完整数字三元组)。
    #[serde(default)]
    pub min_engine_version: Option<String>,
}

/// `major.minor.patch` 朴素三元组解析 (纯数字, 恒 3 段; `1`/`1.2`/`1.x.0` 均非法)。
fn semver_tuple(s: &str) -> Option<(u64, u64, u64)> {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut t = [0u64; 3];
    for (slot, p) in t.iter_mut().zip(&parts) {
        *slot = p.parse().ok()?;
    }
    Some((t[0], t[1], t[2]))
}

/// 兼容性判定核心 (manifest 文本 → 版本协商): 目录式 [check] 与内存源式
/// [load_rules_from_sources](crate::load_rules_from_sources) 共用的同一条
/// 判定路径 (spec §1.M 语义 SSOT — 两形态装载不得分叉)。origin 仅进解析失败
/// 文案的定位 (目录路径 / 内存源名); 版本违例文案与来源形态无关。
pub fn negotiate(raw: &str, engine_version: &str, origin: &str) -> Result<(), String> {
    let m: RulesetManifest =
        serde_yaml::from_str(raw).map_err(|e| format!("manifest: 解析失败 ({origin}): {e}"))?;
    if m.format_version > FORMAT_VERSION {
        return Err(format!(
            "manifest: format_version {} 超出本引擎支持的版本 {} — 规则集来自更新的规则格式, 请升级引擎 (或回退规则集版本)",
            m.format_version, FORMAT_VERSION
        ));
    }
    if let Some(min) = &m.min_engine_version {
        let need = semver_tuple(min).ok_or_else(|| {
            format!(
                "manifest: min_engine_version \"{min}\" 非法 (须 major.minor.patch 完整数字三元组, 如 0.2.0)"
            )
        })?;
        // 引擎版本号来自 CARGO_PKG_VERSION, 构建系统恒保证三元组形态; 解析失败视作构建问题。
        let have = semver_tuple(engine_version).unwrap_or_else(|| {
            panic!("引擎版本号畸形: {engine_version} (CARGO_PKG_VERSION 契约被破坏)")
        });
        if have < need {
            return Err(format!(
                "manifest: min_engine_version {min} 高于本引擎 {engine_version} — 该规则集需要更新的引擎, 请升级引擎 (或回退规则集版本)"
            ));
        }
    }
    Ok(())
}

/// 兼容性判定 (装载前 fail-fast): 目录含 `manifest.yaml` 时校验版本协商。
/// 文件不存在 → Ok (向后兼容窗口, 现存规则集零破坏 — spec §1.M)。
/// 读取失败 / 解析失败 / 版本违例 → Err(拒载文案, 自解释)。
/// 判定本体在 [negotiate] (文本式, 与内存源装载共用)。
pub fn check(dir: &Path, engine_version: &str) -> Result<(), String> {
    let path = dir.join(MANIFEST_FILENAME);
    if !path.is_file() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("manifest: 读取失败 ({}): {e}", path.display()))?;
    negotiate(&raw, engine_version, &path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_tuple_forms() {
        assert_eq!(semver_tuple("0.1.0"), Some((0, 1, 0)));
        assert_eq!(semver_tuple("2.10.3"), Some((2, 10, 3)));
        assert_eq!(semver_tuple(" 1.2.3 "), Some((1, 2, 3))); // 容忍首尾空白
        assert_eq!(semver_tuple("1"), None, "缺段非法");
        assert_eq!(semver_tuple("1.2"), None, "缺段非法");
        assert_eq!(semver_tuple("1.2.3.4"), None, "超段非法");
        assert_eq!(semver_tuple("1.x.0"), None, "非数字非法");
        assert_eq!(semver_tuple(""), None);
    }

    #[test]
    fn unknown_field_rejected() {
        let e = serde_yaml::from_str::<RulesetManifest>("format_version: 1\nfoo: 1")
            .expect_err("manifest 未知字段须拒载");
        assert!(e.to_string().contains("foo"), "{e}");
    }
}

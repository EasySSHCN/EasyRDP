//! 批量导入与去重合并（R-20261118-008）。
//!
//! 支持混合批量解析 .rdp（mstsc 导出）与 JSON Profile（单对象或数组），
//! 逐条报告解析错误；导入前按「主机+端口（大小写不敏感）」与现有连接
//! 比对，提供三种合并策略：跳过 / 覆盖 / 两者保留（重名加后缀 "(2)"）。

use crate::profile::SessionProfile;
use crate::rdp_import;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 一条解析成功的待导入候选。
#[derive(Debug, Clone)]
pub struct ImportCandidate {
    /// 来源文件名（仅展示）。
    pub source: String,
    /// JSON 数组内的条目序号；.rdp 文件恒为 0。
    pub entry_index: usize,
    pub profile: SessionProfile,
    /// .rdp 扩展键 `password:s:` 携带的明文密码（导入后由上层 DPAPI 加密存储）。
    pub plaintext_password: Option<String>,
}

/// 一条解析失败的条目（含失败原因）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFailure {
    pub source: String,
    pub entry_index: usize,
    pub reason: String,
}

/// 单文件解析结果（成功候选或逐条失败）。
#[derive(Debug, Clone)]
pub enum ParsedEntry {
    Ok(ImportCandidate),
    Fail(ImportFailure),
}

/// 去重合并策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupStrategy {
    /// ① 跳过重复（保留现有连接）。
    Skip,
    /// ② 覆盖重复（新 profile 替换旧数据；上层保留分组/密码/收藏）。
    Overwrite,
    /// ③ 两者保留（作为新连接加入；重名自动加后缀 "(2)"）。
    KeepBoth,
}

impl DedupStrategy {
    pub fn from_str(s: &str) -> DedupStrategy {
        match s {
            "overwrite" => DedupStrategy::Overwrite,
            "keep_both" => DedupStrategy::KeepBoth,
            _ => DedupStrategy::Skip,
        }
    }
}

/// 计划动作：对每条候选，上层应如何落库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanAction {
    /// 新增。
    Add,
    /// 与现有键重复且策略为跳过。
    Skip,
    /// 覆盖现有连接（existing = 现有列表索引）。
    Overwrite { existing: usize },
    /// 两者保留：作为新连接加入，name 已按需加 "(2)" 后缀。
    KeepBoth { name: String },
}

/// 合并报告：成功/跳过/覆盖/失败统计 + 失败明细。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MergeReport {
    pub added: usize,
    pub skipped: usize,
    pub overwritten: usize,
    pub failed: usize,
    pub failures: Vec<String>,
}

/// 去重判定键：主机（小写）+ 端口。
pub fn dedup_key(host: &str, port: u16) -> String {
    format!("{}:{}", host.trim().to_ascii_lowercase(), port)
}

/// 解析单个 .rdp 文件为候选（host 为空即视为缺 Computer 字段失败）。
pub fn parse_rdp_file(path: &Path) -> ParsedEntry {
    let source = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let fail = |reason: String| ParsedEntry::Fail(ImportFailure { source: source.clone(), entry_index: 0, reason });
    let lines = match rdp_import::read_lines(path) {
        Ok(l) => l,
        Err(e) => return fail(e.to_string()),
    };
    let profile = rdp_import::parse_lines(&lines);
    if profile.host.trim().is_empty() {
        return fail("缺少 Computer 字段（full address）".to_string());
    }
    if let Err(errs) = profile.validate() {
        return fail(errs.join("; "));
    }
    ParsedEntry::Ok(ImportCandidate {
        source,
        entry_index: 0,
        profile,
        plaintext_password: rdp_import::extract_plaintext_password(&lines),
    })
}

/// 把 JSON 文本解析为候选列表：单对象或数组均可，逐条报告错误。
///
/// 字段兼容：`Computer`/`Full Address`（大小写不敏感）映射为 `host`，
/// `Server Port` 映射为 `port`；顶层 `password` 收集为明文密码。
pub fn parse_json_text(text: &str, source: &str) -> Vec<ParsedEntry> {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return vec![ParsedEntry::Fail(ImportFailure {
                source: source.to_string(),
                entry_index: 0,
                reason: format!("JSON 非法: {e}"),
            })]
        }
    };
    let items: Vec<(usize, &serde_json::Value)> = match &value {
        serde_json::Value::Array(arr) => arr.iter().enumerate().collect(),
        single => vec![(0, single)],
    };
    items
        .into_iter()
        .map(|(i, v)| parse_json_entry(v, source, i))
        .collect()
}

/// 解析单个 JSON 条目（对象字段名规范化为小写后交给 serde）。
fn parse_json_entry(v: &serde_json::Value, source: &str, index: usize) -> ParsedEntry {
    let fail = |reason: String| {
        ParsedEntry::Fail(ImportFailure {
            source: source.to_string(),
            entry_index: index,
            reason,
        })
    };
    let obj = match v.as_object() {
        Some(o) => o,
        None => return fail("条目不是 JSON 对象".to_string()),
    };
    let mut norm = serde_json::Map::new();
    let mut plaintext_password = None;
    for (k, val) in obj {
        let lk = k.trim().to_ascii_lowercase();
        match lk.as_str() {
            "computer" | "full address" | "fulladdress" => {
                norm.insert("host".into(), val.clone());
            }
            "server port" => {
                norm.insert("port".into(), val.clone());
            }
            "password" => {
                if let Some(s) = val.as_str() {
                    plaintext_password = Some(s.to_string());
                } else {
                    return fail("字段类型错误: password 须为字符串".to_string());
                }
            }
            _ => {
                norm.insert(lk, val.clone());
            }
        }
    }
    let profile: SessionProfile = match serde_json::from_value(serde_json::Value::Object(norm)) {
        Ok(p) => p,
        Err(e) => return fail(format!("字段类型错误: {e}")),
    };
    if profile.host.trim().is_empty() {
        return fail("缺少 Computer 字段".to_string());
    }
    if let Err(errs) = profile.validate() {
        return fail(errs.join("; "));
    }
    ParsedEntry::Ok(ImportCandidate {
        source: source.to_string(),
        entry_index: index,
        profile,
        plaintext_password,
    })
}

/// 解析单个 JSON 文件（单对象或数组）。
pub fn parse_json_file(path: &Path) -> Vec<ParsedEntry> {
    let source = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let fail = |reason: String| {
        vec![ParsedEntry::Fail(ImportFailure {
            source: source.clone(),
            entry_index: 0,
            reason,
        })]
    };
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return fail(format!("读取文件失败: {e}")),
    };
    // UTF-16LE BOM 兼容（记事本另存为 Unicode 的情况）
    let text = if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xFE {
        let u16s: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        match String::from_utf16(&u16s) {
            Ok(s) => s,
            Err(e) => return fail(format!("文件不是合法 UTF-16: {e}")),
        }
    } else {
        match String::from_utf8(raw) {
            Ok(s) => s,
            Err(_) => return fail("文件不是合法 UTF-8 文本".to_string()),
        }
    };
    parse_json_text(&text, &source)
}

/// 批量解析：.rdp 与 .json 混选；失败条目不全量失败，逐条报告。
pub fn parse_paths(paths: &[std::path::PathBuf]) -> Vec<ParsedEntry> {
    let mut out = Vec::new();
    for p in paths {
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "json" => out.extend(parse_json_file(p)),
            _ => out.push(parse_rdp_file(p)),
        }
    }
    out
}

/// 生成合并计划：对现有连接列表与候选逐条判定 Add/Skip/Overwrite/KeepBoth。
///
/// existing 为现有连接的 profile 视图；候选与候选之间也可能重复
/// （第二个重复候选按同一策略处理，视同与现有连接重复）。
pub fn plan_merge(
    existing: &[SessionProfile],
    incoming: &[ImportCandidate],
    strategy: DedupStrategy,
) -> Vec<PlanAction> {
    // 不断累积的键 -> 结果列表位置（覆盖不新增键；新增/两者保留追加键，
    // 后续候选与本次新增项重复时同样按策略处理）
    let mut keys: Vec<String> = existing
        .iter()
        .map(|p| dedup_key(&p.host, p.port))
        .collect();
    let mut names: Vec<String> = existing
        .iter()
        .map(|p| display_name(p))
        .collect();
    let mut plan = Vec::with_capacity(incoming.len());
    for cand in incoming {
        let key = dedup_key(&cand.profile.host, cand.profile.port);
        if let Some(_idx) = keys.iter().position(|k| *k == key) {
            match strategy {
                DedupStrategy::Skip => plan.push(PlanAction::Skip),
                DedupStrategy::Overwrite => plan.push(PlanAction::Overwrite {
                    existing: keys.iter().position(|k| *k == key).unwrap(),
                }),
                // 两者保留：键重复也作为新连接加入；仅当显示名重名时加 "(2)" 后缀
                DedupStrategy::KeepBoth => {
                    let renamed = unique_name(display_name(&cand.profile), &names);
                    plan.push(PlanAction::KeepBoth { name: renamed.clone() });
                    keys.push(key);
                    names.push(renamed);
                }
            }
        } else {
            plan.push(PlanAction::Add);
            keys.push(key);
            names.push(display_name(&cand.profile));
        }
    }
    plan
}

/// 重名自动加后缀 "(2)"、"(3)"…（大小写不敏感比对）。
fn unique_name(name: String, taken: &[String]) -> String {
    if !taken
        .iter()
        .any(|n| n.eq_ignore_ascii_case(&name))
    {
        return name;
    }
    for n in 2..=1000 {
        let candidate = format!("{name} ({n})");
        if !taken.iter().any(|t| t.eq_ignore_ascii_case(&candidate)) {
            return candidate;
        }
    }
    format!("{name} (导入副本)")
}

/// 对纯 profile 列表执行合并（core 层测试用；上层 SavedConnection 场景
/// 直接使用 [`plan_merge`] 以便保留 id/分组/密码/收藏）。
pub fn merge_profiles(
    existing: &mut Vec<SessionProfile>,
    incoming: &[ImportCandidate],
    strategy: DedupStrategy,
) -> MergeReport {
    let plan = plan_merge(existing, incoming, strategy);
    let mut report = MergeReport::default();
    for (cand, action) in incoming.iter().zip(plan) {
        match action {
            PlanAction::Add => {
                let mut p = cand.profile.clone();
                p.name = display_name(&cand.profile);
                existing.push(p);
                report.added += 1;
            }
            PlanAction::Skip => report.skipped += 1,
            PlanAction::Overwrite { existing: idx } => {
                let name = existing[idx].name.clone();
                existing[idx] = cand.profile.clone();
                existing[idx].name = name; // 覆盖数据但保留原显示名
                report.overwritten += 1;
            }
            PlanAction::KeepBoth { name } => {
                let mut p = cand.profile.clone();
                p.name = name;
                existing.push(p);
                report.added += 1;
            }
        }
    }
    report
}

fn display_name(p: &SessionProfile) -> String {
    if p.name.trim().is_empty() {
        p.host.clone()
    } else {
        p.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(host: &str, port: u16, name: &str) -> ImportCandidate {
        ImportCandidate {
            source: "t".into(),
            entry_index: 0,
            profile: SessionProfile {
                name: name.into(),
                host: host.into(),
                port,
                ..SessionProfile::default()
            },
            plaintext_password: None,
        }
    }

    #[test]
    fn dedup_key_is_case_insensitive() {
        assert_eq!(dedup_key("RDP.Corp.COM", 3389), dedup_key("rdp.corp.com", 3389));
        assert_ne!(dedup_key("h", 3389), dedup_key("h", 3390));
    }

    // -----------------------------------------------------------------
    // 去重策略三分支
    // -----------------------------------------------------------------

    fn existing() -> Vec<SessionProfile> {
        vec![SessionProfile {
            name: "旧连接".into(),
            host: "rdp.corp.com".into(),
            port: 3389,
            ..SessionProfile::default()
        }]
    }

    #[test]
    fn strategy_skip_marks_duplicate() {
        let ex = existing();
        let inc = vec![cand("RDP.CORP.com", 3389, "新连接")]; // 大小写不同仍判重
        let plan = plan_merge(&ex, &inc, DedupStrategy::Skip);
        assert_eq!(plan, vec![PlanAction::Skip]);
        let mut ex2 = ex.clone();
        let r = merge_profiles(&mut ex2, &inc, DedupStrategy::Skip);
        assert_eq!((r.added, r.skipped, r.overwritten), (0, 1, 0));
        assert_eq!(ex2.len(), 1);
    }

    #[test]
    fn strategy_overwrite_replaces_profile() {
        let mut ex = existing();
        let mut new_p = SessionProfile::default();
        new_p.host = "rdp.corp.com".into();
        new_p.port = 3389;
        new_p.username = "newuser".into();
        new_p.name = "新连接".into();
        let inc = vec![ImportCandidate {
            source: "t".into(),
            entry_index: 0,
            profile: new_p,
            plaintext_password: None,
        }];
        let plan = plan_merge(&ex, &inc, DedupStrategy::Overwrite);
        assert_eq!(plan, vec![PlanAction::Overwrite { existing: 0 }]);
        let r = merge_profiles(&mut ex, &inc, DedupStrategy::Overwrite);
        assert_eq!((r.added, r.skipped, r.overwritten), (0, 0, 1));
        assert_eq!(ex.len(), 1, "覆盖不新增条目");
        assert_eq!(ex[0].username, "newuser");
        assert_eq!(ex[0].name, "旧连接", "覆盖保留原显示名");
    }

    #[test]
    fn strategy_keep_both_adds_with_suffix() {
        let mut ex = existing();
        let inc = vec![cand("rdp.corp.com", 3389, "旧连接")]; // 同名 + 同键
        let plan = plan_merge(&ex, &inc, DedupStrategy::KeepBoth);
        assert_eq!(plan, vec![PlanAction::KeepBoth { name: "旧连接 (2)".into() }]);
        let r = merge_profiles(&mut ex, &inc, DedupStrategy::KeepBoth);
        assert_eq!(r.added, 1);
        assert_eq!(ex.len(), 2);
        assert_eq!(ex[1].name, "旧连接 (2)");
        // 连续两次同名导入依次得 "(2)"、"(3)"
        let r2 = merge_profiles(&mut ex, &inc, DedupStrategy::KeepBoth);
        assert_eq!(r2.added, 1);
        assert_eq!(ex[2].name, "旧连接 (3)");
    }

    #[test]
    fn keep_both_different_name_no_suffix() {
        let ex = existing();
        let inc = vec![cand("rdp.corp.com", 3389, "别名")];
        let plan = plan_merge(&ex, &inc, DedupStrategy::KeepBoth);
        assert_eq!(plan, vec![PlanAction::KeepBoth { name: "别名".into() }]);
    }

    #[test]
    fn non_duplicate_key_always_adds() {
        let ex = existing();
        let inc = vec![cand("other.host", 3390, "x")];
        for s in [DedupStrategy::Skip, DedupStrategy::Overwrite, DedupStrategy::KeepBoth] {
            assert_eq!(plan_merge(&ex, &inc, s), vec![PlanAction::Add], "{s:?}");
        }
    }

    #[test]
    fn duplicate_within_batch_follows_same_strategy() {
        let ex: Vec<SessionProfile> = vec![];
        let inc = vec![cand("h", 3389, "a"), cand("H", 3389, "b")];
        let plan = plan_merge(&ex, &inc, DedupStrategy::Skip);
        assert_eq!(plan, vec![PlanAction::Add, PlanAction::Skip]);
        let plan = plan_merge(&ex, &inc, DedupStrategy::KeepBoth);
        // 同键不同名：不重名则不加后缀
        assert_eq!(plan, vec![PlanAction::Add, PlanAction::KeepBoth { name: "b".into() }]);
        // 同键同名才加后缀
        let inc2 = vec![cand("h", 3389, "a"), cand("H", 3389, "A")];
        let plan = plan_merge(&ex, &inc2, DedupStrategy::KeepBoth);
        assert_eq!(
            plan,
            vec![PlanAction::Add, PlanAction::KeepBoth { name: "A (2)".into() }]
        );
    }

    // -----------------------------------------------------------------
    // 批量解析：合法 / 非法 / 混合；JSON 数组；字段类型错；缺 Computer
    // -----------------------------------------------------------------

    use std::io::Write;

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn parse_rdp_valid_and_invalid_mixed() {
        let ok = write_temp(
            "batch-ok.rdp",
            b"full address:s:rdp.corp.example.com\r\nserver port:i:3390\r\nusername:s:alice\r\n",
        );
        let bad_host = write_temp("batch-nohost.rdp", b"screen mode id:i:2\r\n");
        let bad_file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test-tmp")
            .join("batch-missing.rdp");
        let out = parse_paths(&[ok, bad_host, bad_file]);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], ParsedEntry::Ok(_)));
        match &out[1] {
            ParsedEntry::Fail(f) => assert!(f.reason.contains("Computer"), "{}", f.reason),
            _ => panic!("缺 host 应失败"),
        }
        match &out[2] {
            ParsedEntry::Fail(f) => assert!(f.reason.contains("读取文件失败"), "{}", f.reason),
            _ => panic!("缺文件应失败"),
        }
    }

    #[test]
    fn parse_json_array_mixed_entries() {
        let text = r#"[
            { "name": "a", "Computer": "H1", "port": 3389, "username": "u1" },
            { "name": "bad", "host": 123 },
            { "port": 4000 },
            { "name": "c", "host": "h3", "port": 3391 }
        ]"#;
        let out = parse_json_text(text, "mixed.json");
        assert_eq!(out.len(), 4);
        match &out[0] {
            ParsedEntry::Ok(c) => {
                assert_eq!(c.profile.host, "H1");
                assert_eq!(c.profile.port, 3389);
                assert_eq!(c.entry_index, 0);
            }
            _ => panic!("第 1 条应成功（Computer 别名映射）"),
        }
        match &out[1] {
            ParsedEntry::Fail(f) => assert!(f.reason.contains("字段类型错误"), "{}", f.reason),
            _ => panic!("port 类型错应失败"),
        }
        match &out[2] {
            ParsedEntry::Fail(f) => assert!(f.reason.contains("Computer"), "{}", f.reason),
            _ => panic!("缺 host 应失败"),
        }
        match &out[3] {
            ParsedEntry::Ok(c) => assert_eq!(c.entry_index, 3),
            _ => panic!("第 4 条应成功"),
        }
    }

    #[test]
    fn parse_json_single_object_and_invalid_json() {
        let out = parse_json_text(r#"{ "host": "solo", "port": 3389 }"#, "one.json");
        assert!(matches!(out[0], ParsedEntry::Ok(_)));
        let out = parse_json_text("{ not json", "broken.json");
        match &out[0] {
            ParsedEntry::Fail(f) => assert!(f.reason.contains("JSON 非法")),
            _ => panic!("非法 JSON 应失败"),
        }
    }

    #[test]
    fn parse_json_password_collected_and_rdp_password_extension() {
        let out = parse_json_text(r#"{ "host": "h", "password": "pw123" }"#, "pw.json");
        match &out[0] {
            ParsedEntry::Ok(c) => assert_eq!(c.plaintext_password.as_deref(), Some("pw123")),
            _ => panic!("应成功"),
        }
        let p = write_temp(
            "rdp-pw.rdp",
            b"full address:s:h1\r\npassword:s:plain-pw\r\n",
        );
        match parse_rdp_file(&p) {
            ParsedEntry::Ok(c) => assert_eq!(c.plaintext_password.as_deref(), Some("plain-pw")),
            _ => panic!("rdp password 扩展键应被收集"),
        }
    }

    #[test]
    fn parse_paths_dispatches_by_extension() {
        let rdp = write_temp("m.rdp", b"full address:s:rh1\r\n");
        let js = write_temp(
            "m.json",
            br#"[{ "host": "jh1" }, { "host": "jh2", "port": 3390 }]"#,
        );
        let out = parse_paths(&[rdp, js]);
        assert_eq!(out.len(), 3, "1 个 .rdp + JSON 数组 2 条");
        assert!(out.iter().all(|e| matches!(e, ParsedEntry::Ok(_))));
    }
}

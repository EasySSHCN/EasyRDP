//! 分组树模型（F1-V1「收藏与分组增强」R-20261118-007）。
//!
//! 支持父子层级（最多 3 层）、预设 8 色色板着色、同级排序（order），
//! 并提供落盘前校验：父分组必须存在、父链不许成环、深度不得超限。
//! 旧版 connections.json / groups.json（无 parent/color/order 字段）由
//! serde default 兼容读取（等价于根节点扁平分组）。

use serde::{Deserialize, Serialize};

/// 分组最大嵌套深度（1 = 顶层）。
pub const MAX_DEPTH: usize = 3;

/// 预设分组色板（8 色）。前端色板与此保持一致。
pub const PALETTE: [&str; 8] = [
    "#ef4444", // 红
    "#f59e0b", // 琥珀
    "#22c55e", // 绿
    "#06b6d4", // 青
    "#3b82f6", // 蓝
    "#8b5cf6", // 紫
    "#ec4899", // 粉
    "#64748b", // 灰蓝
];

/// 一个分组节点（groups.json 数组元素）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupNode {
    /// 稳定 id（前端生成，如 `grp-<nanos>`）。
    pub id: String,
    /// 分组显示名。
    pub name: String,
    /// 父分组 id；None = 顶层分组。旧数据缺省 None（扁平兼容）。
    pub parent: Option<String>,
    /// 颜色标记（[`PALETTE`] 之一，小写十六进制）；None = 无色。
    pub color: Option<String>,
    /// 同级排序序号（小在前）。
    pub order: i64,
}

impl Default for GroupNode {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            parent: None,
            color: None,
            order: 0,
        }
    }
}

impl GroupNode {
    /// 颜色是否为色板合法值（大小写不敏感）。
    pub fn is_valid_color(c: &str) -> bool {
        PALETTE.contains(&c.to_ascii_lowercase().as_str())
    }

    /// 规范化颜色（小写；非法值置 None）。
    pub fn normalize_color(&mut self) {
        self.color = self
            .color
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty() && Self::is_valid_color(c))
            .map(|c| c.to_ascii_lowercase());
    }

    /// 计算本节点深度（顶层 = 1）。父链缺失/成环时返回 None（由
    /// [`validate_groups`] 统一报错，此处仅作辅助查询）。
    pub fn depth_in(&self, all: &[GroupNode]) -> Option<usize> {
        let mut cur = self;
        let mut seen = 0usize;
        loop {
            seen += 1;
            if seen > all.len() + 1 {
                return None; // 成环
            }
            match &cur.parent {
                None => return Some(seen),
                Some(pid) => {
                    cur = all.iter().find(|g| &g.id == pid)?;
                }
            }
        }
    }

    /// 本节点是否为 `ancestor_id` 的后代（含成环保护：沿父链最多走
    /// `all.len()` 步）。重挂父前调用，避免把分组挂到自己的子树里。
    pub fn is_descendant_of(&self, all: &[GroupNode], ancestor_id: &str) -> bool {
        let mut cur_id = self.id.as_str();
        for _ in 0..all.len() {
            let node = match all.iter().find(|g| g.id == cur_id) {
                Some(n) => n,
                None => return false,
            };
            match &node.parent {
                Some(pid) if pid == ancestor_id => return true,
                Some(pid) => cur_id = pid.as_str(),
                None => return false,
            }
        }
        false
    }
}

/// 校验分组树合法性。返回全部错误（空 = 通过）。
///
/// 规则：
/// 1. id 非空且全局唯一；
/// 2. name 非空；
/// 3. parent 指向存在的分组；
/// 4. 父链不成环（A→B→A）；
/// 5. 深度 ≤ [`MAX_DEPTH`]；
/// 6. color 为空或属色板。
pub fn validate_groups(groups: &[GroupNode]) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        if g.id.trim().is_empty() {
            errors.push(format!("分组[{i}] id 不能为空"));
        }
        if g.name.trim().is_empty() {
            errors.push(format!("分组[{i}]({}) name 不能为空", g.id));
        }
        if let Some(pid) = &g.parent {
            if pid.trim().is_empty() {
                errors.push(format!("分组 {} parent 为空白（应为 None）", g.id));
            } else if !groups.iter().any(|x| &x.id == pid) {
                errors.push(format!("分组 {} 的父分组 {} 不存在", g.id, pid));
            }
        }
        if let Some(c) = &g.color {
            if !c.is_empty() && !GroupNode::is_valid_color(c) {
                errors.push(format!("分组 {} 颜色 {} 不在预设色板", g.id, c));
            }
        }
    }
    // id 重复检测
    for (i, g) in groups.iter().enumerate() {
        if groups.iter().take(i).any(|x| x.id == g.id) && !g.id.is_empty() {
            errors.push(format!("分组 id 重复: {}", g.id));
        }
    }
    // 成环 + 深度检测（仅在父分组均存在时才有意义）
    if errors.is_empty() {
        for g in groups {
            match g.depth_in(groups) {
                None => errors.push(format!(
                    "分组 {} 父链成环（{} → …）",
                    g.id,
                    g.parent.clone().unwrap_or_default()
                )),
                Some(d) if d > MAX_DEPTH => errors.push(format!(
                    "分组 {} 嵌套深度 {d} 超过上限 {MAX_DEPTH}",
                    g.id
                )),
                Some(_) => {}
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(id: &str, name: &str, parent: Option<&str>) -> GroupNode {
        GroupNode {
            id: id.into(),
            name: name.into(),
            parent: parent.map(|s| s.into()),
            ..GroupNode::default()
        }
    }

    #[test]
    fn valid_three_level_tree_passes() {
        let groups = vec![
            g("a", "公司", None),
            g("b", "研发", Some("a")),
            g("c", "后端", Some("b")),
        ];
        assert_eq!(groups[2].depth_in(&groups), Some(3));
        assert!(validate_groups(&groups).is_ok());
    }

    #[test]
    fn depth_four_fails() {
        let groups = vec![
            g("a", "L1", None),
            g("b", "L2", Some("a")),
            g("c", "L3", Some("b")),
            g("d", "L4", Some("c")),
        ];
        let errs = validate_groups(&groups).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("超过上限")), "{errs:?}");
    }

    #[test]
    fn parent_cycle_fails() {
        let groups = vec![
            GroupNode { parent: Some("b".into()), ..g("a", "A", None) },
            GroupNode { parent: Some("a".into()), ..g("b", "B", None) },
        ];
        let errs = validate_groups(&groups).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("成环")), "{errs:?}");
    }

    #[test]
    fn self_parent_fails() {
        let groups = vec![GroupNode { parent: Some("a".into()), ..g("a", "A", None) }];
        assert!(validate_groups(&groups).is_err());
    }

    #[test]
    fn missing_parent_fails() {
        let groups = vec![g("a", "A", Some("ghost"))];
        let errs = validate_groups(&groups).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("不存在")), "{errs:?}");
    }

    #[test]
    fn duplicate_and_empty_ids_fail() {
        let groups = vec![g("a", "A", None), g("a", "A2", None), g("", "空id", None)];
        let errs = validate_groups(&groups).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("重复")));
        assert!(errs.iter().any(|e| e.contains("id 不能为空")));
    }

    #[test]
    fn descendant_check_blocks_reparent_into_own_subtree() {
        let groups = vec![
            g("a", "A", None),
            g("b", "B", Some("a")),
            g("c", "C", Some("b")),
        ];
        // 拖拽防护语义：把 X 挂到 T 下前检查 T 是否是 X 的后代
        // （此处 T=c 是 X=a 的后代 → 拒绝）
        assert!(groups[2].is_descendant_of(&groups, "a"));
        // 合法方向：a 挂到 c 下（c 不是 a 的后代）
        assert!(!groups[0].is_descendant_of(&groups, "c"));
        assert!(!groups[0].is_descendant_of(&groups, "b"));
    }

    #[test]
    fn color_palette_validation() {
        assert!(GroupNode::is_valid_color("#EF4444"));
        assert!(GroupNode::is_valid_color("#64748b"));
        assert!(!GroupNode::is_valid_color("#123456"));
        assert!(!GroupNode::is_valid_color("red"));
        assert_eq!(PALETTE.len(), 8);
        let mut n = GroupNode { color: Some("#8B5CF6".into()), ..g("a", "A", None) };
        n.normalize_color();
        assert_eq!(n.color.as_deref(), Some("#8b5cf6"));
        n.color = Some("#badbad".into());
        n.normalize_color();
        assert_eq!(n.color, None);
    }

    /// 旧版 groups.json 兼容：无 parent/color/order 字段可读取，
    /// 且等价于顶层扁平分组（校验通过）。
    #[test]
    fn serde_backward_compat_old_group_json() {
        let old = r#"[{ "id": "g1", "name": "客户A" }]"#;
        let groups: Vec<GroupNode> = serde_json::from_str(old).expect("旧 JSON 应可解析");
        assert_eq!(groups[0].parent, None);
        assert_eq!(groups[0].color, None);
        assert_eq!(groups[0].order, 0);
        assert!(validate_groups(&groups).is_ok());
    }

    /// 新字段完整回环：parent/color/order 序列化后读回一致。
    #[test]
    fn serde_roundtrip_with_nesting_and_color() {
        let groups = vec![
            GroupNode { color: Some("#22c55e".into()), order: 2, ..g("a", "A", None) },
            g("b", "B", Some("a")),
        ];
        let json = serde_json::to_string(&groups).unwrap();
        let back: Vec<GroupNode> = serde_json::from_str(&json).unwrap();
        assert_eq!(groups, back);
        assert_eq!(back[1].depth_in(&back), Some(2));
    }
}

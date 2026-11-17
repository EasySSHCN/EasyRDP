//! 文本 -> VK 键入序列生成（方案 B：无 CLIPRDR 时的剪贴板降级通道）。
//!
//! 纯函数模块，不依赖 FreeRDP：default 构建即可编译与单测。
//!
//! α 版策略（对照任务说明的实现决策）：
//! - ASCII 文本逐字转 `Press { vk, shift }` 序列（US 101 布局）；
//!   大写字母/上档符号标记 `shift=true`（按下顺序：Shift 按下 -> 键按下 ->
//!   键释放 -> Shift 释放，由 [`typing_steps_to_key_events`] 展开）。
//! - `\n` -> VK_RETURN、`\t` -> VK_TAB、空格 -> VK_SPACE。
//! - **非 ASCII（含中文）α 版不支持逐字键入**：产 [`TypeStep::Unsupported`]，
//!   转事件时跳过；正确路径是 CLIPRDR CF_UNICODETEXT 通道（send_clipboard_text
//!   主路径），调用方据此向用户提示「该字符需剪贴板通道」。

/// 一条键入步骤：`vk` 为 Windows 虚拟键码，`shift` 表示需要 Shift 修饰。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeStep {
    Press { vk: u16, shift: bool },
    /// 无法用 US 布局键入的字符（α 版：跳过并上报）。
    Unsupported(char),
}

/// 把文本逐字转成键入步骤。
pub fn text_to_typing_steps(text: &str) -> Vec<TypeStep> {
    let mut steps = Vec::new();
    for ch in text.chars() {
        if let Some((vk, shift)) = char_to_vk(ch) {
            steps.push(TypeStep::Press { vk, shift });
        } else {
            steps.push(TypeStep::Unsupported(ch));
        }
    }
    steps
}

/// 单字符 -> (VK, 是否需要 Shift)。None 表示 US 布局无法键入（非 ASCII）。
pub fn char_to_vk(ch: char) -> Option<(u16, bool)> {
    let c = ch;
    // 控制字符子集
    if c == '\n' || c == '\r' {
        return Some((0x0D, false)); // VK_RETURN
    }
    if c == '\t' {
        return Some((0x09, false)); // VK_TAB
    }
    // 字母：小写直接字符键；大写加 Shift
    if c.is_ascii_lowercase() {
        return Some((c as u16, false));
    }
    if c.is_ascii_uppercase() {
        return Some((c as u16, true));
    }
    // 数字：无 Shift
    if c.is_ascii_digit() {
        return Some((c as u16, false));
    }
    if c == ' ' {
        return Some((0x20, false)); // VK_SPACE
    }
    // 标点（US 101 布局）：VK 来自 <winuser.h> VK_OEM_* 常量
    Some(match c {
        '`' => (0xC0, false), // VK_OEM_3   ` ~
        '~' => (0xC0, true),
        '-' => (0xBD, false), // VK_OEM_MINUS - _
        '_' => (0xBD, true),
        '=' => (0xBB, false), // VK_OEM_PLUS  = +
        '+' => (0xBB, true),
        '[' => (0xDB, false), // VK_OEM_4     [ {
        '{' => (0xDB, true),
        ']' => (0xDD, false), // VK_OEM_6     ] }
        '}' => (0xDD, true),
        '\\' => (0xDC, false), // VK_OEM_5    \ |
        '|' => (0xDC, true),
        ';' => (0xBA, false), // VK_OEM_1     ; :
        ':' => (0xBA, true),
        '\'' => (0xDE, false), // VK_OEM_7    ' "
        '"' => (0xDE, true),
        ',' => (0xBC, false), // VK_OEM_COMMA , <
        '<' => (0xBC, true),
        '.' => (0xBE, false), // VK_OEM_PERIOD . >
        '>' => (0xBE, true),
        '/' => (0xBF, false), // VK_OEM_2     / ?
        '?' => (0xBF, true),
        // 数字行的上档符号：Shift + 数字键
        '!' => (0x31, true),
        '@' => (0x32, true),
        '#' => (0x33, true),
        '$' => (0x34, true),
        '%' => (0x35, true),
        '^' => (0x36, true),
        '&' => (0x37, true),
        '*' => (0x38, true),
        '(' => (0x39, true),
        ')' => (0x30, true),
        _ => return None, // 非 ASCII（中文等）：α 不支持
    })
}

/// 键入步骤 -> [`crate::InputEvent`] 序列。
/// Shift 按下 -> 键按下 -> 键释放 -> Shift 释放；Unsupported 步骤被跳过。
pub fn typing_steps_to_key_events(steps: &[TypeStep]) -> Vec<crate::InputEvent> {
    use crate::InputEvent;
    let mut events = Vec::new();
    for step in steps {
        let TypeStep::Press { vk, shift } = step else {
            continue;
        };
        if *shift {
            events.push(InputEvent::Key { code: "VK:0x10".into(), down: true }); // VK_SHIFT
        }
        events.push(InputEvent::Key { code: format!("VK:0x{vk:02X}"), down: true });
        events.push(InputEvent::Key { code: format!("VK:0x{vk:02X}"), down: false });
        if *shift {
            events.push(InputEvent::Key { code: "VK:0x10".into(), down: false });
        }
    }
    events
}

/// 统计无法键入的字符（供上层提示）。
pub fn unsupported_chars(text: &str) -> Vec<char> {
    text.chars().filter(|c| char_to_vk(*c).is_none()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InputEvent;

    #[test]
    fn lowercase_ascii_types_without_shift() {
        let steps = text_to_typing_steps("abc");
        assert_eq!(
            steps,
            vec![
                TypeStep::Press { vk: 0x61, shift: false },
                TypeStep::Press { vk: 0x62, shift: false },
                TypeStep::Press { vk: 0x63, shift: false },
            ]
        );
        // 展开成按下/释放事件对
        let ev = typing_steps_to_key_events(&steps);
        assert_eq!(ev.len(), 6);
        assert_eq!(ev[0], InputEvent::Key { code: "VK:0x61".into(), down: true });
        assert_eq!(ev[1], InputEvent::Key { code: "VK:0x61".into(), down: false });
    }

    #[test]
    fn uppercase_letters_need_shift_press_release_bracket() {
        let steps = text_to_typing_steps("Ab");
        assert_eq!(
            steps,
            vec![
                TypeStep::Press { vk: 0x41, shift: true },
                TypeStep::Press { vk: 0x62, shift: false },
            ]
        );
        let ev = typing_steps_to_key_events(&steps);
        // A: Shift down, A down, A up, Shift up；b: down, up
        assert_eq!(ev.len(), 6);
        assert_eq!(ev[0], InputEvent::Key { code: "VK:0x10".into(), down: true });
        assert_eq!(ev[1], InputEvent::Key { code: "VK:0x41".into(), down: true });
        assert_eq!(ev[2], InputEvent::Key { code: "VK:0x41".into(), down: false });
        assert_eq!(ev[3], InputEvent::Key { code: "VK:0x10".into(), down: false });
        assert_eq!(ev[4], InputEvent::Key { code: "VK:0x62".into(), down: true });
    }

    #[test]
    fn punctuation_maps_shift_states() {
        let steps = text_to_typing_steps("!a@1");
        assert_eq!(
            steps,
            vec![
                TypeStep::Press { vk: 0x31, shift: true }, // ! = Shift+1
                TypeStep::Press { vk: 0x61, shift: false },
                TypeStep::Press { vk: 0x32, shift: true }, // @ = Shift+2
                TypeStep::Press { vk: 0x31, shift: false },
            ]
        );
        // 常用 OEM 键位
        assert_eq!(char_to_vk('-'), Some((0xBD, false)));
        assert_eq!(char_to_vk('_'), Some((0xBD, true)));
        assert_eq!(char_to_vk(':'), Some((0xBA, true)));
        assert_eq!(char_to_vk('?'), Some((0xBF, true)));
    }

    #[test]
    fn control_chars_and_non_ascii_policy() {
        // 换行 -> VK_RETURN；制表 -> VK_TAB
        assert_eq!(char_to_vk('\n'), Some((0x0D, false)));
        assert_eq!(char_to_vk('\t'), Some((0x09, false)));
        // 中文：α 版标记 Unsupported（正确路径是 CLIPRDR UnicodeText 通道）
        let steps = text_to_typing_steps("a中b");
        assert_eq!(steps[1], TypeStep::Unsupported('中'));
        let ev = typing_steps_to_key_events(&steps);
        // Unsupported 被跳过：只有 a、b 各 2 个事件
        assert_eq!(ev.len(), 4);
        assert_eq!(unsupported_chars("你好!"), vec!['你', '好']);
    }

    #[test]
    fn digits_and_space_type_directly() {
        let steps = text_to_typing_steps("9 ");
        assert_eq!(
            steps,
            vec![
                TypeStep::Press { vk: 0x39, shift: false },
                TypeStep::Press { vk: 0x20, shift: false },
            ]
        );
    }
}

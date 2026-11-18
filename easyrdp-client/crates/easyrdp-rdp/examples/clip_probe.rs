//! 无头 CLIPRDR 双向剪贴板专项探针（CLIPBUG-20261117）。
//!
//! 流程：读连接存储第一条连接（密码 DPAPI 解密，不回显）→ 连接真实主机 →
//!   1. 等 8s（通道握手完成，插桩日志落盘）
//!   2. 本地→远端：send_clipboard_text("CLIPTEST-中文English-1234567890")
//!   3. 观测 20s：统计 ClipboardText 事件（远端→本地）与其他事件
//!   4. 打印 DONE；引擎侧细节看 %APPDATA%\EasyRDP\debug-*.log 的 [cliprdr] 行
//!
//! 运行（PowerShell，release 实测）：
//!   $env:PKG_CONFIG_PATH = 'B:\tools\EasyRDP\vcpkg\installed\x64-windows-static-md\lib\pkgconfig'
//!   $env:PATH = 'B:\tools\EasyRDP\pkgconf-msi\PFiles64\pkgconf-3.0.10;' + $env:PATH
//!   cargo run --release -p easyrdp-rdp --features native --example clip_probe
use std::path::PathBuf;
use std::time::{Duration, Instant};

use easyrdp_core::profile::SessionProfile;
use easyrdp_rdp::{create_session, event_channel::FramebufferEvent, native_ready, RdpSession};

fn load_first_connection() -> Result<(SessionProfile, String), String> {
    let path = std::env::var("APPDATA")
        .map(|d| PathBuf::from(d).join("cn.easyrdp.client\\connections.json"))
        .map_err(|_| "APPDATA 环境变量不存在".to_string())?;
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    #[derive(serde::Deserialize)]
    struct Saved {
        #[serde(default)]
        profile: SessionProfile,
        #[serde(default)]
        password: String,
    }
    let saved: Saved = match serde_json::from_str::<Vec<Saved>>(&raw) {
        Ok(list) => list.into_iter().next().ok_or("connections.json 为空数组")?,
        Err(_) => serde_json::from_str::<Saved>(&raw).map_err(|e| format!("解析失败: {e}"))?,
    };
    Ok((saved.profile, saved.password))
}

fn main() {
    assert!(native_ready(), "必须以 --features native 构建");
    let (profile, stored_password) = match load_first_connection() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("[clip-probe] {e}");
            std::process::exit(2);
        }
    };
    // 密码解密（DPAPI，与 e2e_connect 同语义）
    let password = if let Ok(p) = std::env::var("EASYRDP_TEST_PASSWORD") {
        p
    } else if stored_password.is_empty() {
        eprintln!("[clip-probe] 存储密码为空（请在 GUI 保存密码或设 EASYRDP_TEST_PASSWORD）");
        std::process::exit(3);
    } else {
        match easyrdp_core::secret::unprotect(&stored_password) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[clip-probe] 密码解密失败: {e}");
                std::process::exit(3);
            }
        }
    };
    println!("[clip-probe] 目标 {}:{} user={}（密码不回显）", profile.host, profile.port, profile.username);

    let mut session = create_session();
    session.set_password(&password);
    session.connect(&profile).expect("connect 启动失败");
    println!("[clip-probe] 已发起连接，等 12s 让通道握手/登录完成…");
    std::thread::sleep(Duration::from_secs(12));

    let test_text = "CLIPTEST-中文English-1234567890";
    println!("[clip-probe] 发送本地→远端文本（{} chars）", test_text.chars().count());
    match session.send_clipboard_text(test_text) {
        Ok(()) => println!("[clip-probe] send_clipboard_text OK（远端侧按 Ctrl+V 应可粘贴）"),
        Err(e) => println!("[clip-probe] send_clipboard_text 失败: {}", e.0),
    }

    // -- 远端侧复制测试：Win+R → notepad → 输入 → Ctrl+A → Ctrl+C --
    // 若远端 rdpclip 活着，复制会触发服务端 FormatList（引擎日志可证）；
    // 若依旧零下行 → 远端剪贴板栈未启动（rdpclip 死亡/被策略禁用）。
    use easyrdp_rdp::InputEvent as IE;
    macro_rules! key {
        ($s:expr, $vk:expr, $down:expr) => {
            let _ = $s.send_input(&IE::Key { code: format!("VK:0x{:02X}", $vk), down: $down });
        };
    }
    macro_rules! combo {
        ($s:expr, $vks:expr) => {{
            for &v in $vks { key!($s, v, true); }
            std::thread::sleep(Duration::from_millis(80));
            for &v in $vks.iter().rev() { key!($s, v, false); }
        }};
    }
    std::thread::sleep(Duration::from_secs(1));
    combo!(session, &[0x5B, 0x52]); // Win+R
    std::thread::sleep(Duration::from_millis(1500));
    for (ch, vk) in [('n', 0x4E), ('o', 0x4F), ('t', 0x54), ('e', 0x45), ('p', 0x50), ('a', 0x41), ('d', 0x44)] {
        key!(session, vk, true);
        key!(session, vk, false);
        std::thread::sleep(Duration::from_millis(30));
    }
    key!(session, 0x0D, true); // Enter
    key!(session, 0x0D, false);
    std::thread::sleep(Duration::from_secs(3));
    let text = "clip-probe-remote-copy-测试文本";
    // 输入 ASCII 部分（中文走不了键入，复制测试用 ASCII 足够）
    for b in "clip-probe-remote-copy-123456".chars() {
        if let Some(vk) = vk_from_ascii(b) {
            key!(session, vk, true);
            key!(session, vk, false);
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    combo!(session, &[0x11, 0x41]); // Ctrl+A
    std::thread::sleep(Duration::from_millis(300));
    combo!(session, &[0x11, 0x43]); // Ctrl+C（远端复制！）
    println!("[clip-probe] 已在远端模拟复制，观测 15s 看服务端 FormatList…");

    // -- 阶段3：远端手动重启 rdpclip（Win+R 两次），再测远端复制 --
    std::thread::sleep(Duration::from_secs(15));
    println!("[clip-probe] 阶段3：重启远端 rdpclip…");
    combo!(session, &[0x5B, 0x52]); // Win+R
    std::thread::sleep(Duration::from_millis(1200));
    for c in "taskkill /f /im rdpclip.exe".chars() {
        if let Some(vk) = vk_from_ascii(c) {
            key!(session, vk, true);
            key!(session, vk, false);
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    key!(session, 0x0D, true);
    key!(session, 0x0D, false);
    std::thread::sleep(Duration::from_millis(2000));
    key!(session, 0x1B, true); // ESC 关掉可能的错误弹窗
    key!(session, 0x1B, false);
    std::thread::sleep(Duration::from_millis(500));
    combo!(session, &[0x5B, 0x52]); // Win+R
    std::thread::sleep(Duration::from_millis(1200));
    for c in "rdpclip".chars() {
        if let Some(vk) = vk_from_ascii(c) {
            key!(session, vk, true);
            key!(session, vk, false);
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    key!(session, 0x0D, true);
    key!(session, 0x0D, false);
    std::thread::sleep(Duration::from_secs(3));
    // 再测远端复制
    combo!(session, &[0x5B, 0x52]);
    std::thread::sleep(Duration::from_millis(1200));
    for c in "notepad".chars() {
        if let Some(vk) = vk_from_ascii(c) {
            key!(session, vk, true);
            key!(session, vk, false);
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    key!(session, 0x0D, true);
    key!(session, 0x0D, false);
    std::thread::sleep(Duration::from_secs(3));
    for b in "after-rdpclip-restart-987654".chars() {
        if let Some(vk) = vk_from_ascii(b) {
            key!(session, vk, true);
            key!(session, vk, false);
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    std::thread::sleep(Duration::from_millis(400));
    combo!(session, &[0x11, 0x41]);
    std::thread::sleep(Duration::from_millis(300));
    combo!(session, &[0x11, 0x43]);
    println!("[clip-probe] rdpclip 重启后再次远端复制，观测 15s…");
    let _ = text;

    // 观测 20s：远端→本地方向靠 ClipboardText 事件
    let rx = session.events().map(|r| r.clone_receiver());
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut clip_events = 0usize;
    let mut other_events = 0usize;
    while Instant::now() < deadline {
        match rx.as_ref().and_then(|r| r.try_recv()) {
            Some(FramebufferEvent::ClipboardText(t)) => {
                clip_events += 1;
                println!(
                    "[clip-probe] 远端→本地 ClipboardText: {} chars（前20: {:?}）",
                    t.chars().count(),
                    t.chars().take(20).collect::<String>()
                );
            }
            Some(_) => other_events += 1,
            None => std::thread::sleep(Duration::from_millis(300)),
        }
    }
    println!("[clip-probe] 观测结束: ClipboardText={clip_events} 其他事件={other_events}");
    let _ = session.disconnect();
    println!("[clip-probe] DONE——引擎侧细节看 %APPDATA%\\EasyRDP\\debug-<pid>.log 的 [cliprdr] 行");
}

fn vk_from_ascii(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 0x41,
        '0'..='9' => c as u8 - b'0' + 0x30,
        '-' => 0xBD,
        '/' => 0xBF,
        '.' => 0xBE,
        _ => return None,
    })
}

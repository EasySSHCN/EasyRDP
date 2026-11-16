//! 链接级 + 行为级验证（无服务器依赖，可在本机跑）：
//! 1. freerdp_new/free 创建与释放（DLL 加载验证）。
//! 2. 未连接状态调用 send_input：必须返回 Err 且不 panic（UAF 防护检查）。
//! 3. 连接 127.0.0.1 随机端口（必失败）：验证 connect 失败事件路径
//!    （事件通道收到 Disconnected），并验证失败后 send_input 仍安全报错。
//!
//! 运行示例（PowerShell，需先设 env，见 docs/WINDOWS-FREERDP-SETUP.md）：
//!   $env:PKG_CONFIG_PATH = 'B:\tools\EasyRDP\vcpkg\installed\x64-windows\lib\pkgconfig'
//!   $env:PATH = 'B:\tools\EasyRDP\pkgconf-msi\PFiles64\pkgconf-3.0.10;B:\tools\EasyRDP\vcpkg\installed\x64-windows\bin;' + $env:PATH
//!   cargo run -p easyrdp-rdp --features native --example freerdp_probe
use std::time::{Duration, Instant};

use easyrdp_core::profile::{Resolution, SessionProfile};
use easyrdp_rdp::{create_session, native_ready, InputEvent};

fn main() {
    // -- 0. 直接 FFI 链接验证：freerdp_new/free（DLL 加载） --
    unsafe {
        let instance = freerdp2_sys::freerdp_new();
        assert!(!instance.is_null(), "freerdp_new() 返回 NULL，DLL 加载/符号解析失败");
        println!("freerdp_new() -> {:p}", instance);
        freerdp2_sys::freerdp_free(instance);
        println!("freerdp_free() ok — FreeRDP DLL 加载与 FFI 调用验证通过");
    }

    // -- 1. native 引擎就绪 --
    assert!(native_ready(), "本示例必须以 --features native 构建");
    let mut session = create_session();

    // -- 2. 未连接时 send_input：Err 且不 panic --
    let r = session.send_input(&InputEvent::MouseMove { x: 1, y: 2 });
    assert!(r.is_err(), "未连接时 send_input 应返回 Err");
    println!("未连接 send_input -> Err（安全）: {}", r.unwrap_err());

    // -- 3. 连接 127.0.0.1 随机高位端口（无人监听，必失败） --
    let mut profile = SessionProfile::default();
    profile.host = "127.0.0.1".into();
    profile.port = 49152 + (std::process::id() % 10000) as u16; // 49152-59151 动态端口段
    profile.resolution = Resolution::Custom { w: 640, h: 480 };
    session.set_password("unused");
    let connect_result = session.connect(&profile);
    // connect 是异步启动（返回只代表线程已启动），不假定其成功/失败
    println!("connect(127.0.0.1:{}) -> {:?}", profile.port, connect_result.map(|_| "started"));

    // 等待失败路径事件（Disconnected）；上限 15s。
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_disconnected = false;
    if let Some(rx) = session.events() {
        while Instant::now() < deadline {
            match rx.try_recv() {
                Some(easyrdp_rdp::event_channel::FramebufferEvent::Disconnected(reason)) => {
                    println!("收到 Disconnected: {reason}");
                    got_disconnected = true;
                    break;
                }
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
    assert!(
        got_disconnected,
        "15s 内未收到 Disconnected 事件（连接失败路径未闭环）"
    );

    // -- 4. 失败后再发输入：仍 Err 且不 panic --
    let r = session.send_input(&InputEvent::Key { code: "VK:0x41".into(), down: true });
    assert!(r.is_err(), "断开后 send_input 应返回 Err");
    println!("断开后 send_input -> Err（安全）: {}", r.unwrap_err());

    // -- 5. 幂等 disconnect 不 panic --
    session.disconnect().unwrap();
    session.disconnect().unwrap();
    println!("全部验证通过：freerdp 链接 OK、输入通路防护 OK、失败路径事件 OK");

    // -- 6. 协议级验证（真实主机 + 假凭据，NLA/NTLM 路径） --
    // 目标 192.168.200.26:3389 启用 NLA；假凭据 test/test。预期不再是
    // 网络层失败，而是认证相关错误（Disconnected 事件已带
    // freerdp_get_last_error 的数字与字符串）。事件循环跑满 8 秒或
    // 收到 Disconnected 为止。
    let mut profile2 = SessionProfile::default();
    profile2.host = "192.168.200.26".into();
    profile2.port = 3389;
    profile2.username = "test".into();
    profile2.resolution = Resolution::Custom { w: 640, h: 480 };
    let mut probe = create_session();
    probe.set_password("test");
    let started = probe.connect(&profile2);
    println!(
        "[192.168.200.26:3389] connect(test/test) -> {:?}",
        started.map(|_| "started")
    );
    let deadline2 = Instant::now() + Duration::from_secs(8);
    if let Some(rx) = probe.events() {
        while Instant::now() < deadline2 {
            match rx.try_recv() {
                Some(easyrdp_rdp::event_channel::FramebufferEvent::Disconnected(reason)) => {
                    println!("[192.168.200.26] 最终结果 Disconnected: {reason}");
                    break;
                }
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
    let _ = probe.disconnect();
    println!("协议级验证结束");
}

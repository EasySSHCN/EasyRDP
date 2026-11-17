//! Windows 静态链接 FreeRDP 2.11.2 的通道入口桩（α 版取舍）。
//!
//! 背景：Windows 侧 FreeRDP 2.11.2 为静态链接（vcpkg x64-windows-static-md），
//! `freerdp-client2.lib` 内的 tables.c **静态入口表**编译期硬引用全部内置
//! 通道的入口符号（`*_VirtualChannelEntryEx` / `*_DVCPluginEntry` /
//! `*_DeviceServiceEntry` / 各 subsystem entry）。动态链接时这些符号由
//! 独立 DLL 各自携带；静态链接后必须在最终二进制里**逐一提供**，否则链接失败
//! （25 个 unresolved externals）。
//!
//! α 版策略：vendor 真实 cliprdr（剪贴板）+ drdynvc/rdpgfx（RDPEGFX 图形
//! 管线，见 build.rs 方案 A），其余符号全部用本文件的桩满足链接。**桩仅在静态链接期满足符号表，
//! 语义上是"入口存在但通道未启用"**：`VirtualChannelEntryEx` 类返回
//! `CHANNEL_RC_NOT_OPEN` 语义值 1（FreeRDP 加载器会视作通道初始化失败并
//! 忽略该通道，不影响连接）；subsystem/DeviceService 类按其指针语义返回
//! 0（NULL 失败值，避免返回 1 变成悬垂指针）。
//!
//! 取舍说明（α 版）：
//! - drdynvc/rdpgfx 已 vendor 真实现 ⇒ RDPEGFX 动态图形管线可用
//!   （ffi.rs on_pre_connect 开启 SupportGraphicsPipeline，DVC 协商后
//!   SurfaceBits 经 GDI gfx 路径合成，复用既有 BeginPaint/EndPaint 帧管线）。
//!   未启用 H264/AVC444（静态库无解码后端）：rdpgfx 能力通告带
//!   AVC_DISABLED，服务端自动回退 Clear/Planar/RemoteFX uncompressed。
//! - rdpdr 桩 ⇒ 驱动器/打印机/智能卡重定向不可用（ffi.rs 已把这些
//!   Redirect* 开关固定为 FALSE，加载器不会真正调用这些桩）。
//! - rdpsnd/audin 桩 ⇒ 音频重定向不可用（AudioPlayback/AudioCapture 已关）。
//! - rail 桩 ⇒ RemoteApp 不可用；encomsp/echo/geometry/video/disp/ainput 等
//!   同理（disp 桩意味着动态分辨率 DisplayControl 留待 β 版）。
//! - 桩永远不会被调用路径触达：ffi.rs `on_pre_connect` 只开启
//!   RedirectClipboard（真实 cliprdr），其余重定向开关显式关闭，FreeRDP
//!   的 `freerdp_client_load_addins` 不会加载对应通道。保留桩只是让静态
//!   tables.c 的引用可解析。
//!
//! 签名对照 FreeRDP 2.11.2 源码（`channels/<ch>/client/<ch>_main.c`）：
//! - `BOOL VCAPITYPE X_VirtualChannelEntryEx(PCHANNEL_ENTRY_POINTS, PVOID)`
//! - `UINT X_DVCPluginEntry(IDRDYNVC_ENTRY_POINTS*)`
//! - `UINT X_DeviceServiceEntry(PDEVICE_SERVICE_ENTRY_POINTS)`
//! - subsystem entry 见各 `*_winmm.c` / `printer_win.c` / `rdpsnd_fake.c`
//!   的 `FREERDP_ENTRY_POINT` 展开符号名。α 版统一按 `(ptr, ptr) -> u32`
//!   满足符号即可（调用约定 x64 统一，形参未使用）。

use std::os::raw::c_void;

/// `*_VirtualChannelEntryEx` 桩（返回值 1 = CHANNEL_RC_NOT_OPEN 语义，
/// FreeRDP 加载器忽略初始化失败的静态通道）。
macro_rules! stub_channel_entry_ex {
    ($name:ident) => {
        #[no_mangle]
        pub extern "system" fn $name(_p_entry_points: *mut c_void, _p_init: *mut c_void) -> u32 {
            1 // CHANNEL_RC_NOT_OPEN 语义：通道不可用
        }
    };
}

/// `*_DVCPluginEntry` / `*_DeviceServiceEntry` 桩（返回 1 同上）。
macro_rules! stub_dvc_or_device {
    ($name:ident) => {
        #[no_mangle]
        pub extern "system" fn $name(_p_entry_points: *mut c_void) -> u32 {
            1 // CHANNEL_RC_NOT_OPEN 语义：通道不可用
        }
    };
}

/// subsystem entry 桩（rdpsnd/audin/printer 的子系统注册入口返回**指针**，
/// 返回 0 = NULL 失败值；不会被调用路径触达，见模块注释）。
macro_rules! stub_subsystem {
    ($name:ident) => {
        #[no_mangle]
        pub extern "system" fn $name() -> u32 {
            0 // NULL：子系统不可用（指针语义，避免悬垂值）
        }
    };
}

// -- 静态虚拟通道入口（tables.c 静态表引用）--
// cliprdr（VirtualChannelEntryEx）与 drdynvc/rdpgfx（RDPEGFX 图形管线，
// drdynvc_VirtualChannelEntryEx + rdpgfx_DVCPluginEntry）为 vendor 真实现
// （见 build.rs 方案 A），不在桩内。
stub_channel_entry_ex!(remdesk_VirtualChannelEntryEx);
stub_channel_entry_ex!(rdpsnd_VirtualChannelEntryEx);
stub_channel_entry_ex!(rdpdr_VirtualChannelEntryEx);
stub_channel_entry_ex!(rdp2tcp_VirtualChannelEntryEx);
stub_channel_entry_ex!(rail_VirtualChannelEntryEx);
stub_channel_entry_ex!(encomsp_VirtualChannelEntryEx);

// -- 动态虚拟通道插件（经 drdynvc 真实管理器按需加载）--
// rdpgfx_DVCPluginEntry 已 vendor 真实现；drdynvc 加载 DVC 时经
// freerdp_load_channel_addin_entry 解析静态表符号，未 vendor 的 DVC 仍由
// 下方桩满足符号（返回 1 = 初始化失败，drdynvc 忽略该插件）。
stub_dvc_or_device!(video_DVCPluginEntry);
stub_dvc_or_device!(rdpsnd_DVCPluginEntry);
stub_dvc_or_device!(rdpei_DVCPluginEntry);
stub_dvc_or_device!(geometry_DVCPluginEntry);
stub_dvc_or_device!(echo_DVCPluginEntry);
stub_dvc_or_device!(disp_DVCPluginEntry);
stub_dvc_or_device!(audin_DVCPluginEntry);
stub_dvc_or_device!(ainput_DVCPluginEntry);

// -- rdpdr 设备服务（rdpdr 已桩化，设备服务不会被加载）--
stub_dvc_or_device!(smartcard_DeviceServiceEntry);
stub_dvc_or_device!(printer_DeviceServiceEntry);
stub_dvc_or_device!(drive_DeviceServiceEntry);

// -- subsystem 入口（rdpsnd / audin 子系统、printer win 子系统；指针语义返回 NULL）--
stub_subsystem!(winmm_freerdp_rdpsnd_client_subsystem_entry);
stub_subsystem!(winmm_freerdp_audin_client_subsystem_entry);
stub_subsystem!(win_freerdp_printer_client_subsystem_entry);
stub_subsystem!(fake_freerdp_rdpsnd_client_subsystem_entry);

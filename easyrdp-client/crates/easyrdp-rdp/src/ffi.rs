//! FreeRDP FFI 集成（仅在 `--features native` 下编译并链接）。
//!
//! 绑定来源：[`freerdp2_sys`] 0.2（bindgen 0.65 生成，链接 `freerdp2 >= 2.4` /
//! `freerdp-client2` / `winpr2`，经 pkg-config/system-deps 定位）。
//!
//! ============ α 版范围与已知限制 ============
//!
//! - **无 user_data 的回调**：FreeRDP 2.x 的 `pPreConnect`/`pPostConnect`
//!   只带 `freerdp*` 一个参数，Rust 侧通过 [`EVENT_ROUTES`]
//!   （instance 指针地址 -> 事件 Sender 的全局注册表）路由事件。
//!   这是 α 版的已知妥协，β 版应改为扩大 `ContextSize` 并在自定义
//!   context 头部存放 `Arc<SessionShared>`。
//! - **事件循环为轮询**：α 版用 `freerdp_check_event_handles` + 20ms sleep
//!   驱动；正确做法是 `freerdp_get_event_handles` + `WaitForMultipleObjects`
//!   （winpr），避免空转。
//! - **像素格式透传**：`BITMAP_DATA.bitmapDataStream` 原样推给上层，
//!   未做 RLE 解压 / BGRA->RGBA / GDI 合成（上层拿到的是服务端位图字节）。
//! - **密码注入**：`SessionProfile` 当前没有 password 字段，密码经
//!   [`FreerdpSession::set_password`] 在 connect 前注入 settings；
//!   未提供密码时依赖 `instance.Authenticate` 回调（α 版固定回 FALSE，
//!   即密码缺失直接连接失败，不弹交互）。
//! - **忽略证书**：α 版固定 `FreeRDP_IgnoreCertificate = TRUE`（内网验收用）。
//!   β 版必须改为 `VerifyX509Certificate` 回调 + 指纹白名单。
//! - **未实现**：虚拟通道（RDPDR/CLIPRDR/RDPSND）、RD Gateway、
//!   NLA 凭据交互（CredSSP prompting）、动态分辨率（DisplayControl）、
//!   SurfaceBits(RDP8 graphics) 通路。
//!
//! 每个 `unsafe` 块都极小化并注明对应 FreeRDP C API 与头文件来源。
//! 符号名与结构体字段均已对照 freerdp2-sys 0.2.0 的 bindgen 输出核实
//! （`freerdp` 为 `rdp_freerdp` 的别名、`rdpContext` 为 `rdp_context` 的别名，
//! settings id 常量为 `u32`、setter 形参为 `usize`）。

#![cfg(feature = "native")]

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use freerdp2_sys as sys;

use easyrdp_core::profile::{Resolution, SessionProfile};

use crate::event_channel::{self, FramebufferEvent};
use crate::{RdpError, RdpSession};

/// FreeRDP BOOL 的 Rust 常量（`sys::BOOL = i32`，见 `<winpr/wtypes.h>`）。
const TRUE: sys::BOOL = 1;
const FALSE: sys::BOOL = 0;

/// instance 指针地址 -> 事件发送端。
///
/// FreeRDP 2.x 的连接/更新回调只带 `freerdp*`（无 user_data），
/// 用全局注册表把 C 回调路由回对应会话的事件通道。
/// 键为 `instance as usize`；会话 Drop/disconnect 时注销。
static EVENT_ROUTES: Mutex<Option<HashMap<usize, event_channel::Sender>>> = Mutex::new(None);

fn route_register(instance_addr: usize, tx: event_channel::Sender) {
    EVENT_ROUTES
        .lock()
        .expect("EVENT_ROUTES poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(instance_addr, tx);
}

fn route_unregister(instance_addr: usize) {
    if let Some(map) = EVENT_ROUTES.lock().expect("EVENT_ROUTES poisoned").as_mut() {
        map.remove(&instance_addr);
    }
}

fn route_lookup(instance_addr: usize) -> Option<event_channel::Sender> {
    EVENT_ROUTES
        .lock()
        .expect("EVENT_ROUTES poisoned")
        .as_ref()
        .and_then(|m| m.get(&instance_addr).cloned())
}

// ---------------------------------------------------------------------------
// extern "C" 回调（注册进 FreeRDP instance / update 结构）
// ---------------------------------------------------------------------------

/// `pPreConnect`（C 原型见 `<freerdp/freerdp.h>`：`typedef BOOL (*pPreConnect)(freerdp*)`）。
///
/// α 版固定返回 TRUE。β 版在此创建 CLIPRDR/RDPDR 等虚拟通道。
unsafe extern "C" fn on_pre_connect(_instance: *mut sys::freerdp) -> sys::BOOL {
    TRUE
}

/// `pPostConnect`（`<freerdp/freerdp.h>`：`typedef BOOL (*pPostConnect)(freerdp*)`）。
///
/// 注册 update 回调（BitmapUpdate / PointerPosition），α 版占位完成。
/// 注意：本回调在 FreeRDP 内部线程被调用，只做指针写入，不触碰 Rust 堆对象。
unsafe extern "C" fn on_post_connect(instance: *mut sys::freerdp) -> sys::BOOL {
    let ctx = (*instance).context;
    if ctx.is_null() {
        return FALSE;
    }
    // rdpContext.update: *mut rdpUpdate（`<freerdp/update.h>`）
    let update = (*ctx).update;
    if !update.is_null() {
        (*update).BitmapUpdate = Some(on_bitmap_update);
        // rdpUpdate.pointer: *mut rdpPointerUpdate
        let pointer = (*update).pointer;
        if !pointer.is_null() {
            (*pointer).PointerPosition = Some(on_pointer_position);
        }
    }
    TRUE
}

/// `pBitmapUpdate`（`<freerdp/update.h>`：
/// `typedef BOOL (*pBitmapUpdate)(rdpContext*, const BITMAP_UPDATE*)`）。
///
/// 把每个 `BITMAP_DATA` 脏矩形转成 [`FramebufferEvent::Framebuffer`] 推入通道。
/// 像素按服务端原始格式透传（α 限制，见模块注释）。
unsafe extern "C" fn on_bitmap_update(ctx: *mut sys::rdpContext, bitmap: *const sys::BITMAP_UPDATE) -> sys::BOOL {
    if ctx.is_null() || bitmap.is_null() || (*bitmap).rectangles.is_null() {
        return FALSE;
    }
    let instance = (*ctx).instance;
    let Some(tx) = route_lookup(instance as usize) else {
        return TRUE; // 未注册路由（如已 disconnect）：静默丢弃，不中断 FreeRDP
    };
    let count = (*bitmap).number.min((*bitmap).count) as usize;
    let rects = std::slice::from_raw_parts((*bitmap).rectangles, count);
    for r in rects {
        // _BITMAP_DATA 字段见 `<freerdp/update.h>`（destLeft/destTop/destRight/destBottom/
        // width/height/bitsPerPixel/bitmapLength/bitmapDataStream）
        let w = r.destRight.saturating_sub(r.destLeft).saturating_add(1);
        let h = r.destBottom.saturating_sub(r.destTop).saturating_add(1);
        let pixels = if r.bitmapDataStream.is_null() || r.bitmapLength == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(r.bitmapDataStream, r.bitmapLength as usize).to_vec()
        };
        let _ = tx.send(FramebufferEvent::Framebuffer {
            x: r.destLeft,
            y: r.destTop,
            width: w,
            height: h,
            pixels,
        });
    }
    TRUE
}

/// `pPointerPosition`（`<freerdp/update.h>`：
/// `typedef BOOL (*pPointerPosition)(rdpContext*, const POINTER_POSITION_UPDATE*)`）。
unsafe extern "C" fn on_pointer_position(
    ctx: *mut sys::rdpContext,
    pos: *const sys::POINTER_POSITION_UPDATE,
) -> sys::BOOL {
    if ctx.is_null() || pos.is_null() {
        return FALSE;
    }
    if let Some(tx) = route_lookup((*ctx).instance as usize) {
        let _ = tx.send(FramebufferEvent::Pointer {
            x: (*pos).xPos,
            y: (*pos).yPos,
        });
    }
    TRUE
}

/// `pAuthenticate`（`<freerdp/freerdp.h>`）。α 版：密码已在 settings 里，
/// 缺失即失败；β 版在此实现交互式凭据获取。
unsafe extern "C" fn on_authenticate(
    _instance: *mut sys::freerdp,
    _username: *mut *mut ::std::os::raw::c_char,
    _password: *mut *mut ::std::os::raw::c_char,
    _domain: *mut *mut ::std::os::raw::c_char,
) -> sys::BOOL {
    FALSE
}

// ---------------------------------------------------------------------------
// 极小化 unsafe 封装：每个函数对应一个 FreeRDP C API
// ---------------------------------------------------------------------------

/// 包装 `freerdp_new()`（`<freerdp/freerdp.h>`）：创建 freerdp instance。
unsafe fn ffi_instance_new() -> *mut sys::freerdp {
    sys::freerdp_new()
}

/// 包装 `freerdp_free()`（`<freerdp/freerdp.h>`）：释放 instance 本体。
/// 必须先 `freerdp_context_free`。
unsafe fn ffi_instance_free(instance: *mut sys::freerdp) {
    sys::freerdp_free(instance)
}

/// 包装 `freerdp_context_new()`（`<freerdp/freerdp.h>`）：
/// 分配 rdpContext 并初始化 settings/input/update 等子结构。
unsafe fn ffi_context_new(instance: *mut sys::freerdp) -> sys::BOOL {
    sys::freerdp_context_new(instance)
}

/// 包装 `freerdp_context_free()`（`<freerdp/freerdp.h>`）。
unsafe fn ffi_context_free(instance: *mut sys::freerdp) {
    sys::freerdp_context_free(instance)
}

/// 包装 `freerdp_connect()`（`<freerdp/freerdp.h>`）：阻塞式建立连接。
unsafe fn ffi_connect(instance: *mut sys::freerdp) -> sys::BOOL {
    sys::freerdp_connect(instance)
}

/// 包装 `freerdp_disconnect()`（`<freerdp/freerdp.h>`）：优雅断开（GracefulDisconnect
/// 序列由 FreeRDP 内部完成），返回 FALSE 时连接可能已异常中断。
unsafe fn ffi_disconnect(instance: *mut sys::freerdp) -> sys::BOOL {
    sys::freerdp_disconnect(instance)
}

/// 包装 `freerdp_abort_connect()`（`<freerdp/freerdp.h>`，返回 BOOL）：
/// 触发 abortEvent，让阻塞中的 connect/事件循环尽快返回。
/// 返回值仅指示信号置位成功与否，α 版忽略（连接终止由事件循环确认）。
unsafe fn ffi_abort_connect(instance: *mut sys::freerdp) {
    let _ = sys::freerdp_abort_connect(instance);
}

/// 包装 `freerdp_shall_disconnect()`（`<freerdp/freerdp.h>`）：
/// 服务端/本地任一方请求断开时返回 TRUE。
unsafe fn ffi_shall_disconnect(instance: *mut sys::freerdp) -> bool {
    sys::freerdp_shall_disconnect(instance) != 0
}

/// 包装 `freerdp_check_event_handles()`（`<freerdp/freerdp.h>`）：
/// 处理一次就绪的通道/socket 事件（α 版轮询入口）。
/// 返回 FALSE 表示连接错误（`context->LastError` 持有 `FREERDP_ERROR_*`）。
unsafe fn ffi_check_event_handles(ctx: *mut sys::rdpContext) -> sys::BOOL {
    sys::freerdp_check_event_handles(ctx)
}

// -- settings（`<freerdp/settings.h>`；id 为 FreeRDP_* 常量，形参 usize） --

/// 包装 `freerdp_settings_set_string()`（`<freerdp/settings.h>`）。
/// FreeRDP 会复制字符串，CString 可在调用后立即释放。
unsafe fn settings_set_string(
    settings: *mut sys::rdpSettings,
    id: u32,
    value: &str,
) -> Result<(), RdpError> {
    let c = CString::new(value).map_err(|_| RdpError(format!("settings 字符串含 \\0: {id}")))?;
    if sys::freerdp_settings_set_string(settings, id as usize, c.as_ptr()) == 0 {
        return Err(RdpError(format!("freerdp_settings_set_string 失败: id={id}")));
    }
    Ok(())
}

/// 包装 `freerdp_settings_set_uint32()`（`<freerdp/settings.h>`）。
unsafe fn settings_set_uint32(settings: *mut sys::rdpSettings, id: u32, value: u32) -> Result<(), RdpError> {
    if sys::freerdp_settings_set_uint32(settings, id as usize, value) == 0 {
        return Err(RdpError(format!("freerdp_settings_set_uint32 失败: id={id}")));
    }
    Ok(())
}

/// 包装 `freerdp_settings_set_bool()`（`<freerdp/settings.h>`）。
unsafe fn settings_set_bool(settings: *mut sys::rdpSettings, id: u32, value: bool) -> Result<(), RdpError> {
    if sys::freerdp_settings_set_bool(settings, id as usize, if value { TRUE } else { FALSE }) == 0
    {
        return Err(RdpError(format!("freerdp_settings_set_bool 失败: id={id}")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// FreerdpSession
// ---------------------------------------------------------------------------

/// 由 [`FreerdpSession`] 拥有并最终释放的 FreeRDP instance/context 指针。
///
/// α 版采用“settings 先同步写入、connect 与事件循环在专用线程”的结构：
/// `freerdp_connect` 是阻塞调用，不能在 UI/调用线程上执行。
struct OwnedInstance {
    ptr: *mut sys::freerdp,
}
// SAFETY：instance 指针本身只是地址；真正的使用（connect/事件驱动）被限制在
// 单一事件循环线程内（FreeRDP 要求单线程驱动），跨线程仅传递指针所有权。
unsafe impl Send for OwnedInstance {}

/// 真实 RDP 会话（native 编译时由 [`crate::create_session`] 返回）。
pub struct FreerdpSession {
    /// connect 前经 `set_password` 注入；`SessionProfile` 目前无 password 字段。
    password: Option<String>,
    /// 建立成功后持有 instance，disconnect/Drop 时按序释放。
    owned: Option<OwnedInstance>,
    /// 事件循环线程句柄（connect 由它执行）。
    worker: Option<JoinHandle<()>>,
    /// 线程间停止信号。
    stop: Arc<AtomicBool>,
    /// 会话存活标志（对外状态查询）。
    connected: Arc<AtomicBool>,
    /// 事件接收端（供上层 `events()`）。
    receiver: event_channel::Receiver,
}

// SAFETY：与 OwnedInstance 相同的约定 —— FFI 指针只在事件循环线程解引用，
// Rust 侧共享的只有 AtomicBool 与 mpsc 通道（均为线程安全类型）。
unsafe impl Send for FreerdpSession {}

impl FreerdpSession {
    pub fn new() -> Self {
        let (_tx, receiver) = event_channel::channel();
        Self {
            password: None,
            owned: None,
            worker: None,
            stop: Arc::new(AtomicBool::new(false)),
            connected: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    }

    /// 注入口令（在 [`RdpSession::connect`] 之前调用）。
    ///
    /// 背景约束：`SessionProfile` 模型没有 password 字段（口令属于凭据层，
    /// 不落盘到 profile JSON），所以 α 版由上层显式传入。
    pub fn set_password(&mut self, password: impl Into<String>) {
        self.password = Some(password.into());
    }

    /// 把 [`SessionProfile`] 翻译成 FreeRDP settings 键值
    /// （纯函数部分独立成 `plan_settings` 便于文档化/审查，实际写入在
    /// [`apply_profile_settings`]）。
    ///
    /// 映射：
    /// - `host/port/username/password` -> `FreeRDP_ServerHostname/ServerPort/Username/Password`
    /// - `color_depth`                 -> `FreeRDP_ColorDepth`
    /// - `Resolution::FitWindow`       -> 固定 1280x720 起步（α；动态分辨率留待
    ///    DisplayControl/MonitorLayout PDU，见 README 未实现清单）
    /// - `Resolution::Fullscreen`      -> `FreeRDP_Fullscreen = TRUE`（几何由 FreeRDP
    ///    客户端侧按主显示器解析）
    /// - `Resolution::Custom { w, h }` -> `FreeRDP_DesktopWidth/Height`
    pub(crate) fn plan_settings(profile: &SessionProfile) -> Vec<(String, String)> {
        let mut kv = vec![
            ("ServerHostname".to_string(), profile.host.clone()),
            ("ServerPort".to_string(), profile.port.to_string()),
            ("Username".to_string(), profile.username.clone()),
            ("ColorDepth".to_string(), profile.color_depth.to_string()),
            ("IgnoreCertificate".to_string(), "true".into()),
        ];
        match profile.resolution {
            Resolution::FitWindow => {
                kv.push(("DesktopWidth".to_string(), "1280".into()));
                kv.push(("DesktopHeight".to_string(), "720".into()));
            }
            Resolution::Fullscreen => kv.push(("Fullscreen".to_string(), "true".into())),
            Resolution::Custom { w, h } => {
                kv.push(("DesktopWidth".to_string(), w.to_string()));
                kv.push(("DesktopHeight".to_string(), h.to_string()));
            }
        }
        kv
    }
}

impl Default for FreerdpSession {
    fn default() -> Self {
        Self::new()
    }
}

/// 实际写入 settings（对应 `plan_settings` 的注释映射）。
///
/// 前置条件：`instance.context`/`instance.settings` 已由 `freerdp_context_new`
/// 初始化。所有 FFI 细节见上方 `settings_set_*` 包装函数。
unsafe fn apply_profile_settings(
    instance: *mut sys::freerdp,
    profile: &SessionProfile,
    password: Option<&str>,
) -> Result<(), RdpError> {
    let settings = (*instance).settings;
    if settings.is_null() {
        return Err(RdpError("freerdp_context_new 未初始化 settings".into()));
    }

    // FreeRDP_ServerHostname / ServerPort / Username —— `<freerdp/settings.h>`
    settings_set_string(settings, sys::FreeRDP_ServerHostname, profile.host.trim())?;
    settings_set_uint32(settings, sys::FreeRDP_ServerPort, profile.port as u32)?;
    settings_set_string(settings, sys::FreeRDP_Username, &profile.username)?;
    // 口令：profile 不携带，来自凭据层注入（见 set_password）。
    if let Some(pw) = password {
        settings_set_string(settings, sys::FreeRDP_Password, pw)?;
    }
    settings_set_uint32(settings, sys::FreeRDP_ColorDepth, profile.color_depth)?;

    // α 版：跳过证书校验（内网验收）。β 版换 VerifyX509Certificate 回调。
    settings_set_bool(settings, sys::FreeRDP_IgnoreCertificate, true)?;

    // 密码缺失时依赖 Authenticate 回调 —— α 版该回调固定 FALSE，避免交互挂起。
    (*instance).Authenticate = Some(on_authenticate);

    match profile.resolution {
        Resolution::FitWindow => {
            // α 版起步分辨率；动态分辨率（DisplayControl）未实现，见模块注释。
            settings_set_uint32(settings, sys::FreeRDP_DesktopWidth, 1280)?;
            settings_set_uint32(settings, sys::FreeRDP_DesktopHeight, 720)?;
        }
        Resolution::Fullscreen => {
            // FreeRDP_Fullscreen —— `<freerdp/settings.h>`；几何由客户端侧解析主显示器。
            settings_set_bool(settings, sys::FreeRDP_Fullscreen, true)?;
        }
        Resolution::Custom { w, h } => {
            settings_set_uint32(settings, sys::FreeRDP_DesktopWidth, w)?;
            settings_set_uint32(settings, sys::FreeRDP_DesktopHeight, h)?;
        }
    }

    // 重定向开关（α 版默认关；profile.redirect 仅剪贴板默认开）。
    settings_set_bool(settings, sys::FreeRDP_RedirectClipboard, profile.redirect.clipboard)?;
    settings_set_bool(settings, sys::FreeRDP_RedirectPrinters, profile.redirect.printers)?;
    settings_set_bool(settings, sys::FreeRDP_RedirectDrives, profile.redirect.drives)?;
    // 注意：FreeRDP 2.x 没有 FreeRDP_RedirectCameras（摄像头重定向是 3.x 特性），
    // profile.redirect.camera 在 α 版被忽略并在 README 中标注。

    Ok(())
}

impl RdpSession for FreerdpSession {
    fn connect(&mut self, profile: &SessionProfile) -> Result<(), RdpError> {
        if self.owned.is_some() || self.worker.is_some() {
            return Err(RdpError("会话已建立，请先 disconnect".into()));
        }
        if profile.host.trim().is_empty() {
            return Err(RdpError("host 不能为空".into()));
        }

        // -- 阶段 1：创建 instance/context 并注册回调（同步、无 I/O） --
        // SAFETY：freerdp_new 返回裸指针，失败（NULL）立即报错；
        // 后续每一步失败都走 teardown_task 释放。
        let instance = unsafe { ffi_instance_new() };
        if instance.is_null() {
            return Err(RdpError("freerdp_new 失败".into()));
        }
        let mut teardown = Teardown {
            instance,
            done: false,
        };

        // 注册连接状态回调（必须在 freerdp_connect 之前）。
        // SAFETY：仅写入 instance 结构体字段，同线程。
        unsafe {
            (*instance).PreConnect = Some(on_pre_connect);
            (*instance).PostConnect = Some(on_post_connect);
        }

        // SAFETY：freerdp_context_new 要求 instance 由 freerdp_new 创建且未初始化过。
        if unsafe { ffi_context_new(instance) } == 0 {
            teardown.release();
            return Err(RdpError("freerdp_context_new 失败".into()));
        }

        // -- 阶段 2：settings 映射（同步、无 I/O） --
        // SAFETY：context_new 之后 settings 已初始化。
        if let Err(e) = unsafe { apply_profile_settings(instance, profile, self.password.as_deref()) }
        {
            teardown.release();
            return Err(e);
        }

        // -- 阶段 3：注册事件路由 + 事件循环线程（freerdp_connect 在线程内执行） --
        // 本会话的事件通道：Sender 交给 FFI 回调路由表，Receiver 归 self.receiver，
        // 上层经 RdpSession::events() 消费。
        let (tx, rx) = event_channel::channel();
        let instance_addr = instance as usize;
        route_register(instance_addr, tx);

        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let host = profile.host.trim().to_string();

        let owned = OwnedInstance { ptr: instance };
        teardown.done = true; // 释放责任移交给线程（run_session 收尾）

        let stop_t = Arc::clone(&stop);
        let connected_t = Arc::clone(&connected);
        // teardown 所有权移交线程：connect 失败/成功后的释放都在线程内完成，
        // disconnect() 通过 stop + abort_connect 让线程退出并释放。
        let worker = std::thread::spawn(move || {
            run_session(owned, teardown, stop_t, connected_t, instance_addr, host);
        });

        self.owned = Some(OwnedInstance { ptr: instance });
        self.worker = Some(worker);
        self.stop = stop;
        self.connected = connected;
        self.receiver = rx;
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), RdpError> {
        // GracefulDisconnect 顺序（幂等）：
        //   1) stop 标志 + freerdp_abort_connect 唤醒阻塞中的 connect/事件循环
        //   2) join 事件循环线程（线程内：freerdp_disconnect -> 释放 instance）
        //   3) 注销事件路由
        // SAFETY：instance 指针此时仍被 worker 线程使用，这里只解引用它
        // 调 abort_connect（FreeRDP abortEvent 写入是线程安全的）。
        self.stop.store(true, Ordering::SeqCst);
        if let Some(owned) = &self.owned {
            unsafe { ffi_abort_connect(owned.ptr) };
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.owned = None;
        self.connected.store(false, Ordering::SeqCst);
        // 路由注销放在 join 之后，确保线程内不会再往通道发送。
        Ok(())
    }

    fn events(&self) -> Option<&event_channel::Receiver> {
        Some(&self.receiver)
    }
}

impl Drop for FreerdpSession {
    fn drop(&mut self) {
        let _ = self.disconnect();
    }
}

/// 结构化释放守卫：任何一步失败时保证 `freerdp_context_free` + `freerdp_free`
/// 恰好执行一次（对应 C 侧常见的 goto teardown 模式）。
struct Teardown {
    instance: *mut sys::freerdp,
    done: bool,
}

// SAFETY：Teardown 只携带指针地址；其释放只发生在独占该指针的
// 事件循环线程内（run_session 收尾）或 connect 同步路径（无线程并发）。
unsafe impl Send for Teardown {}

impl Teardown {
    /// 立即释放（connect 同步路径失败时）。
    fn release(mut self) {
        self.done = true;
        // SAFETY：instance 已不再被任何线程使用（connect 尚未进入线程阶段）。
        unsafe {
            ffi_context_free(self.instance);
            ffi_instance_free(self.instance);
        }
    }
}

impl Drop for Teardown {
    fn drop(&mut self) {
        if !self.done {
            // SAFETY：与 release 相同的前置条件（同步路径，无并发使用）。
            unsafe {
                ffi_context_free(self.instance);
                ffi_instance_free(self.instance);
            }
        }
    }
}

/// 事件循环线程收尾专用：context_free/disconnect 已在 run_session 内显式完成，
/// 这里只负责 `freerdp_free`（保持恰好一次释放）。
impl Teardown {
    fn free_instance_only(mut self) {
        self.done = true;
        // SAFETY：事件循环线程退出后 instance 无并发使用者。
        unsafe { ffi_instance_free(self.instance) };
    }
}

/// 事件循环线程体：freerdp_connect -> 轮询 freerdp_check_event_handles ->
/// 退出时按序释放（GracefulDisconnect 顺序见 disconnect 注释）。
fn run_session(
    owned: OwnedInstance,
    teardown: Teardown,
    stop: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    instance_addr: usize,
    host: String,
) {
    let instance = owned.ptr;

    // SAFETY：instance 由本线程独占（调用线程已放弃所有权）。
    let ok = unsafe { ffi_connect(instance) } != 0;
    if !ok {
        connected.store(false, Ordering::SeqCst);
        if let Some(tx) = route_lookup(instance_addr) {
            let _ = tx.send(FramebufferEvent::Disconnected(format!(
                "freerdp_connect 到 {host} 失败（检查网络/凭据/证书策略）"
            )));
        }
        route_unregister(instance_addr);
        // connect 失败后仍需完整释放：disconnect（清理半开连接状态，幂等）->
        // context_free -> instance_free（经 free_instance_only 前先显式 context_free）。
        // SAFETY：连接失败路径，无并发使用者。
        unsafe {
            ffi_disconnect(instance);
            ffi_context_free(instance);
        }
        teardown.free_instance_only();
        return;
    }
    connected.store(true, Ordering::SeqCst);

    let ctx = unsafe { (*instance).context };

    // α 版轮询事件循环：CheckFileDescriptor 语义由 freerdp_check_event_handles
    // 承担（内部 select 轮询 socket/通道句柄）。β 版换成
    // freerdp_get_event_handles + WaitForMultipleObjects 以消除 20ms 空转。
    loop {
        if stop.load(Ordering::SeqCst) || unsafe { ffi_shall_disconnect(instance) } {
            break;
        }
        // SAFETY：context 在会话生命周期内有效；本线程是唯一驱动线程。
        let alive = unsafe { ffi_check_event_handles(ctx) } != 0;
        if !alive {
            // PointerUpdate/FrameAcknowledge 等占位处理都已在注册的
            // update 回调里完成；此处只关心连接是否还活着。
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    connected.store(false, Ordering::SeqCst);
    if let Some(tx) = route_lookup(instance_addr) {
        let _ = tx.send(FramebufferEvent::Disconnected(
            if stop.load(Ordering::SeqCst) {
                "本地请求断开".to_string()
            } else {
                "服务端断开或连接错误".to_string()
            },
        ));
    }
    route_unregister(instance_addr);

    // SAFETY：事件循环已退出，本线程是 instance 的最后使用者；
    // freerdp_disconnect（GracefulDisconnect）必须先于 context_free/free。
    unsafe {
        ffi_disconnect(instance);
        ffi_context_free(instance);
    } // freerdp_free 由 free_instance_only 执行（保持恰好一次释放）
    teardown.free_instance_only();
}

// ---------------------------------------------------------------------------
// 单测（仅 native 构建时编译；默认 CI 不执行）
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use easyrdp_core::profile::Resolution;

    #[test]
    fn plan_settings_maps_host_port_and_custom_resolution() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.8".into();
        p.port = 3390;
        p.username = "ops".into();
        p.resolution = Resolution::Custom { w: 1920, h: 1080 };
        let kv = FreerdpSession::plan_settings(&p);
        assert!(kv.contains(&("ServerHostname".to_string(), "10.0.0.8".to_string())));
        assert!(kv.contains(&("ServerPort".to_string(), "3390".to_string())));
        assert!(kv.contains(&("DesktopWidth".to_string(), "1920".to_string())));
    }

    #[test]
    fn plan_settings_fitwindow_uses_alpha_default_size() {
        let mut p = SessionProfile::default();
        p.host = "h".into();
        p.resolution = Resolution::FitWindow;
        let kv = FreerdpSession::plan_settings(&p);
        assert!(kv.contains(&("DesktopWidth".to_string(), "1280".to_string())));
    }
}

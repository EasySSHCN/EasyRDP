# easyrdp-rdp

EasyRDP 远程桌面引擎层。对上层只暴露两个东西：

- [`RdpSession`] trait：`connect` / `disconnect` / `events`
- [`create_session`] 工厂 + [`native_ready`] 能力探测

```
easyrdp-app (Tauri/UI)
      │  只面向 RdpSession trait
      ▼
easyrdp-rdp ──┬── default feature：StubSession（返回 "engine not compiled"，无 FreeRDP 依赖）
              └── native  feature：FreerdpSession（FreeRDP 2.x FFI 真实实现）
```

## Native 构建前提

`native` feature 依赖 crates.io 的 [`freerdp2-sys` 0.2](https://crates.io/crates/freerdp2-sys)
（bindgen 原始绑定，经 `system-deps` + pkg-config 定位库），要求系统已安装
**FreeRDP 2.4+** 开发库（`freerdp2` / `freerdp-client2` / `winpr2` 三个 pkg-config 条目）。

### Debian / Ubuntu

```bash
sudo apt install libfreerdp2-dev libfreerdp-client2-dev libwinpr2-dev pkg-config
# 验证：
pkg-config --modversion freerdp2 freerdp-client2 winpr2   # 应 >= 2.4
```

### Fedora / RHEL

```bash
sudo dnf install freerdp-devel libwinpr-devel pkgconf-pkg-config
```

### macOS（Homebrew）

```bash
brew install freerdp pkg-config
# Homebrew 装的是 FreeRDP 2.x 时可用；3.x 与本 -sys 版本不兼容（见下方风险）
```

### Windows（vcpkg）

```powershell
vcpkg install freerdp:x64-windows-static
# freerdp2-sys 0.2 用 system-deps 7，Windows 下需设置环境变量指向 vcpkg：
setx PKG_CONFIG_PATH "%VCPKG_ROOT%\installed\x64-windows-static\lib\pkgconfig"
```

### 构建

```bash
# 引擎 crate 单独构建（native）：
cargo build -p easyrdp-rdp --features native

# 整个客户端（native 引擎 + 自定义协议）：
cargo build --release -p easyrdp-app --features custom-protocol,native

# 默认构建（不含 native，CI/单测路径，无任何 FreeRDP 依赖）：
cargo build --release -p easyrdp-app --features custom-protocol
```

> **重要**：本仓库开发机未安装 FreeRDP 开发库，`--features native` 的完整编译
> 在接入 CI 或开发机装库前**无法验证**；`ffi.rs` 的符号与类型均对照
> freerdp2-sys 0.2.0 的 bindgen 输出逐项核实，但最终以真实构建为准。

## 已实现（α 版，`--features native`）

- `FreerdpSession::connect`：`freerdp_new` → 回调注册（PreConnect/PostConnect/Authenticate）
  → `freerdp_context_new` → settings 映射 → 专用线程内 `freerdp_connect`
- settings 映射：`ServerHostname/ServerPort/Username/Password/ColorDepth`、
  `IgnoreCertificate = TRUE`（α）、分辨率三态（FitWindow→1280x720 起步 /
  Fullscreen→`FreeRDP_Fullscreen` / Custom→`DesktopWidth/Height`）、重定向开关
  （clipboard/printers/drives；camera 为 FreeRDP 3.x 特性，α 忽略）
- 事件循环：轮询 `freerdp_check_event_handles`（20ms），`freerdp_shall_disconnect` /
  stop 标志 / `freerdp_abort_connect` 三重退出条件
- 帧通路：`rdpUpdate.BitmapUpdate`（BITMAP_DATA 脏矩形 → `FramebufferEvent::Framebuffer`
  推 mpsc channel）+ `rdpPointerUpdate.PointerPosition`（→ `FramebufferEvent::Pointer`）
- `disconnect`：stop → `freerdp_abort_connect` → join 线程 → `freerdp_disconnect`
  → `freerdp_context_free` → `freerdp_free`（Teardown 守卫保证恰好一次释放）
- 口令注入：`FreerdpSession::set_password`（`SessionProfile` 无 password 字段，
  口令属于凭据层，不落 profile JSON）

## 未实现 / β 版待办

- **证书校验**：α 固定 `IgnoreCertificate=TRUE`；β 必须实现
  `VerifyX509Certificate` 回调 + 指纹白名单
- **交互式凭据**：`Authenticate` 回调 α 固定返回 FALSE（无密码即失败）；
  β 接 UI 弹窗，另考虑 CredSSP/NLA 提示
- **动态分辨率**：FitWindow 目前固定 1280x720；需要 DisplayControl /
  MonitorLayout PDU + 窗口尺寸联动
- **像素合成**：`FramebufferEvent::Framebuffer.pixels` 是服务端位图原始字节
  （可能 RLE 压缩、BGRA 序），未接 GDI（`gdi_init`）做解压/格式转换/合成；
  上层渲染前需自行处理，β 版计划在引擎内出 BGRA 全帧
- **事件循环**：轮询改 `freerdp_get_event_handles` + `WaitForMultipleObjects`
  （winpr），消除 20ms 空转
- **虚拟通道**：RDPDR（驱动器/打印机重定向数据面）、CLIPRDR（剪贴板）、
  RDPSND/RDPGFX 均未接入（settings 开关只是声明，无数据面）
- **RD Gateway**：`SessionProfile.gateway` 尾随未消费
- **SurfaceBits**（RDP8 RemoteFX/NSCodec 全帧通路）、`SurfaceFrameAcknowledge`
- **回调上下文**：α 用全局 `EVENT_ROUTES`（instance 指针地址 → Sender）路由
  FreeRDP 回调；β 改扩大 `ContextSize` 在自定义 context 头部放 `Arc<SessionShared>`

## 适配风险（与真实 FreeRDP 的兼容性）

| 风险 | 说明 |
|---|---|
| FreeRDP 3.x 不兼容 | `freerdp2-sys` 锚定 2.4+ 的 C ABI；系统装了 3.x（Homebrew/新版发行版默认）会导致链接或运行时行为漂移，需显式装 2.x |
| `FreeRDP_Password` 明文驻留 | settings 内为明文；长期应改 3.x 的 `FreeRDP_PasswordHash` 或凭据回调即用即弃 |
| bindgen 结构体布局 | `rdp_freerdp`/`rdp_update` 等带 padding 字段，依赖 FreeRDP 2.x 稳定布局；2.x 小版本若改结构体，字段写入会静默错位（回调注册属此类） |
| `system-deps` 版本分歧 | crates.io 发布的 freerdp2-sys 0.2.0 用 `system-deps = "6"`，上游仓库 main 分支已升到 `"7"`；两者对本 crate 均可用，但构建产物（如 Windows 下 pkg-config 探测行为）可能有差异 |
| 回调线程安全 | BitmapUpdate/PointerPosition 回调在 FreeRDP 内部线程执行，α 实现只经 mpsc 发送，不得在回调内触碰 Rust 侧会话对象 |

## 测试

```bash
cargo test --workspace            # default features（CI 路径）
cargo test -p easyrdp-rdp --features native   # 含 plan_settings 映射测试（需本机装 FreeRDP）
```

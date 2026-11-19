# F3-V1 R-20261118-010「多显示器优化」交付报告

日期：2026-11-18 ｜ 范围：开发 + 自测（未 git 提交 / 未发布 / 未部署）

## 一、功能概览

| # | 功能 | 状态 |
|---|------|------|
| 1 | 显示器枚举 `list_monitors()` 命令 + 前端「显示」下拉显示器选择分组 | ✅ |
| 2 | Profile 新增 `monitor: Option<u32>`；全屏窗口 SetWindowPos 定位到所选显示器 | ✅ |
| 3 | Profile 新增 `multi_monitor: bool`；全屏"跨所有显示器"（虚拟屏 rect） | ✅ |
| 4 | custom 分辨率"按所选显示器分辨率填充"按钮 | ✅ |
| 5 | FreeRDP settings 同步：`FreeRDP_SpanMonitors` / `FreeRDP_MonitorIds/NumMonitorIds` | ✅（键名已在 vcpkg 2.11.2 settings.h 核实） |
| 6 | 单测：枚举结构 serde、Profile serde default 兼容、plan_settings 多显示器键、网关覆写既有测试不回归 | ✅ |

## 二、改动文件

| 文件 | 改动 |
|------|------|
| `crates/easyrdp-core/src/profile.rs` | `SessionProfile` 新增 `monitor`/`multi_monitor`（serde 容器 default，旧 JSON 兼容）；新增兼容性单测 |
| `crates/easyrdp-rdp/src/lib.rs` | 新增 `fullscreen_resolution(monitor, span_all)`：虚拟屏/指定显示器几何 → FreeRDP DesktopWidth/Height |
| `crates/easyrdp-rdp/src/ffi.rs` | `plan_settings`/`apply_profile_settings`：Fullscreen 分支按 `multi_monitor` 写 `FreeRDP_SpanMonitors`(387)、按 `monitor` 写 `FreeRDP_MonitorIds`(393)+`NumMonitorIds`(394)；DesktopWidth/Height 改用 `fullscreen_resolution`；新增 `plan_settings_fullscreen_multimon_keys` 单测 |
| `crates/easyrdp-rdp/Cargo.toml` | windows-sys 增加 `Win32_Graphics_Gdi` feature（EnumDisplayMonitors） |
| `src-tauri/src/monitors.rs` | **新增**：`MonitorInfo`/`MonitorsView`/`enumerate_monitors()`/`virtual_screen()`/`fullscreen_window_rect()`/`list_monitors` command + 单测 |
| `src-tauri/src/main.rs` | 注册 `monitors` 模块与 `list_monitors` 命令；`SaveArgs`/`ConnectionView` 新增 monitor/multiMonitor 字段（serde default） |
| `src-tauri/src/session.rs` | fullscreen 模式远程尺寸改用 `fullscreen_resolution`；计算定位 rect 传入 attach；debug.log 记录 |
| `src-tauri/src/rdp_view_final.rs` | `attach_for_session/attach_on_main_thread` 新增 `fullscreen_rect` 参数：窗口直接定位目标显示器/虚拟屏（不缩放不居中）；`FULLSCREEN_RECTS` 表；标题栏最大化按钮在"全屏 rect ↔ 该显示器内 95% 恢复"间切换（ModeSwitch 覆盖）；WM_DESTROY 清理 |
| `src/index.html` | 显示器选择分组 UI（下拉 + 跨屏复选 + 填充按钮）；缓存版本 v=2026111802 |
| `src/ui/main.js` | `loadMonitorOptions()`；multimon 区仅 fullscreen 显示；loadToForm/resetForm 回显复位；saveCurrent 提交 monitor/multiMonitor |
| `src/ui/i18n-dict-zh-CN.js` / `-en.js` | 新增 form.res.fillMonitor / form.label.monitor / form.monitor.* 六键（ko/zh-TW 缺省回退机制覆盖） |

## 三、FreeRDP settings 依据（settings.h 已核实，vcpkg 2.11.2）

- `FreeRDP_SpanMonitors` (387) BOOL — 跨所有显示器（mstsc `/span` 对应）。
- `FreeRDP_MonitorIds` (393, UINT32*) + `FreeRDP_NumMonitorIds` (394) — 指定显示器（指针数组按 FreeRDP 约定不复制，单元素数组刻意保留至会话生命周期）。
- `FreeRDP_Fullscreen` (1537) — 仅客户端窗口模式标志；远程桌面尺寸始终显式写 `DesktopWidth`(129)/`Height`(130)。
- 注：FreeRDP 2.11.2 **无 MultiMonitorCompensate 键**（该补偿逻辑属 wxWidgets/xfreerdp 前端，不适用于本项目自建 GDI 窗口架构）。

## 四、测试结果（实测 2026-11-18）

- `cargo check -p easyrdp-app --features custom-protocol,native`：**通过**（exit 0）。
- `cargo test -p easyrdp-core -p easyrdp-rdp -p easyrdp-app`：**全绿**——
  easyrdp-core 56 passed / easyrdp-rdp 77 passed / easyrdp-app 21 passed，0 failed。
- 定向验证新增测试：
  - `easyrdp-core`：`profile::tests::serde_backward_compat_old_json_without_monitor_fields` ✅
  - `easyrdp-rdp --features native`：`ffi::tests::plan_settings_fullscreen_multimon_keys` ✅（含网关覆写既有测试不回归）
  - `easyrdp-app`：`monitors::tests::enumerate_returns_primary_and_serde_roundtrip` / `fullscreen_window_rect_selection_semantics` ✅
- 说明：实施过程中顺带发现并修复了一次 transient 编译态（自建 windows-sys 0.61
  MONITORENUMPROC 签名为 `(HMONITOR, HDC, *mut RECT, LPARAM)`，与常见文档签名
  `(HDC, HMONITOR, LPARAM)` 不同——两侧枚举回调均按 0.61 真实签名实现）。

## 五、真机验证清单（需双屏以上机器）

1. 前端新建连接表单「显示 → 全屏」出现「目标显示器」下拉：显示器 N（分辨率，主屏标注）；单屏机器该分组隐藏。
2. 选择显示器 2 全屏连接：RDP 窗口完整出现在显示器 2（顶到屏幕边缘），远程桌面分辨率 = 显示器 2 实际分辨率（任务栏不遮挡验证）。
3. 勾选「跨所有显示器」：窗口覆盖虚拟屏整体（含负坐标副屏布局）；远程分辨率 = 虚拟屏整体尺寸。
4. 全屏窗口标题栏最大化按钮：在"全屏 rect ↔ 该显示器内 95% 恢复"间往返切换，不漂移到其他显示器。
5. custom 模式点「按所选显示器分辨率填充」：宽高输入框填为所选显示器尺寸。
6. 编辑旧版连接（connections.json 无 monitor/multi_monitor 字段）：正常加载，缺省主显示器/不跨屏。
7. FreeRDP SpanMonitors 生效性（可选深验）：连接后服务端会话显示多显示器联合桌面（依赖服务端能力，失败时回退单屏分辨率，客户端窗口行为不受影响）。
8. debug.log 检查 `[multimon]` 行：monitor/span/rect/remote 与所选一致。

## 六、遗留项

1. 全屏窗口顶部仍保留 32px 自绘标题栏（工具条），未做"纯全屏无边框"隐藏——后续可按 FULLSCREEN_RECTS 状态在 WM_PAINT/命中测试中抑制。
2. `MonitorIds` 指针数组为一次性泄漏分配（单元素、会话级生命周期）；若后续支持会话热重建需改为随 OwnedInstance 释放。
3. 显示器"友好名称"（如 "DELL U2720Q"）需 QueryDisplayConfig，α 版仅返回 `\\.\DISPLAYN` 设备名 + 序号标注。
4. MonitorIds 生效依赖 FreeRDP 多显示器协商路径（SupportMonitorLayoutPdu/MonitorCount），真机未逐项验证，列入真机清单第 7 条。
5. ko / zh-TW 语言包未加新键（回退机制显示英文/中文默认，可在后续版本补齐）。

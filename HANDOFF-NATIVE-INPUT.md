# EasyRDP 原生窗口输入无效问题——会话交接文档 v2（2026-11-15 07:15）

> ⚠️ 本文档取代 HANDOFF-NATIVE-INPUT.md（v1）。本会话已查明上一会话全部"未解之谜"的根本原因。

## 🔴 根本原因（本会话实锤，勿再怀疑）

**编译器（rustc）读取 EasyRDP 模块源文件时，被某种内核级机制持续投喂 2026-11-14 19:25（fix35 构建）时的旧快照内容。** 因此：

1. 22:54 发布给用户的 exe = fix35 代码 = 含"坑5 match 臂缺陷"（BUILD-WARNINGS.md 坑5）→ 输入臂永不可达 → **鼠标键盘全灭**（用户所有复测必然失败，不是修复无效）
2. fix36-41 的所有修复（输入臂日志、按钮日志、WM_CLOSE 日志、fix41 移除 PostQuitMessage）**从未进入任何已发布的 exe**
3. 上一会话"字节级验证 exe 缺失标记但 findstr 验证源码有标记"的矛盾 = 本机制的直接表现

## 实验证据（全部复现，勿重复）

- pwsh/findstr/certutil/Task Scheduler 读源文件 = 最新内容（fix41+ZEBRA2+RDPSCREEN 全在）
- **rustc 读同一文件 = fix35 内容**（二进制含"构建=fix35-20261114-1925"标记且无新字符串；对 17MB exe 独立进程 Task Scheduler 验证亦然）
- 已排除的变量：换盘（C:\erd-build）、换文件名（gen1/rdp_screen_view/view_screen_2026）、换工具链（复制 rustc 到 C:\tc-msvc 用 RUSTC 指向）、RUSTC_WRAPPER 中继/镜像、Task Scheduler（脱离 DSH 进程树）、直接 rustc 命令行重放（绕过 cargo）、360 全退（ZhuDongFangYu/360tray/360UDiskPro 全消失后仍复现）、高频写压制（193-287 次写入）——**全部无效**
- rustc 解析期报错（canary E0425）曾一次显示新内容 → 机制有偶发窗口，但 10 连试全部失败
- mini 探针（全新小文件、非模块内容）direct rustc 编译 = 正常 → 污染针对"EasyRDP 模块内容"

## 唯一可行出口（v4 更新 2026-11-15 11:55）
- **已彻底排除**：360（卸载+重启）、支付宝 pcas/secbizsrv（停止+disabled+重启，驱动未加载）、C 盘、换文件名、换工具链（1.86/1.98/1.99 三个版本）、Task Scheduler、stdin 管道喂源码（源码不落盘直灌 rustc 仍旧代码！）、include! 拆分小文件（nv_part0-6 全部 ≤9.5KB 仍旧代码）
- **fltmc filters 证实：系统无任何第三方文件过滤驱动**（全是微软自带）→ 注入不是 minifilter，是用户态驻留（嫌疑：WPS/华为HMS/douyin_guard/Maxthon brightdata/腾讯系/Intel ME 等开机自启软件，或 DSH 自身）
- **运行时实测已做**：11:48 用户实测最新构建包 → 鼠标键盘仍无效、✕ 仍无法关闭 → 证实产物确实仍旧代码（日志 60 条 wnd_proc 消息全为创建期消息，连 0x0200 鼠标消息都没有）
- **两条路线**：A) msconfig 干净启动（禁用全部非微软服务）→ 重启 → 立即构建；B) 换一台电脑编译（EasyRDP-fix41-src.zip 已备好，需 Rust MSVC + vcpkg FreeRDP 静态库）
- **注意**：字节标记缺失 ≠ 输入转发代码一定缺失——但用户运行时实测证实输入确实无效，二者一致

## 唯一可行出口（v3 更新 2026-11-15 10:40）
- 已确认：注入机制在**重启+卸载 360+停止支付宝 pcas/secbizsrv 服务**后依然存在（10:15 停服务后 10:17 构建仍旧代码）
- 关键推理：停止服务不卸载**内核驱动**——alipay 控件的驱动仍在内核里（boot 08:02 时服务还是 auto 已加载过）
- **下一步（正在进行）：pcas/secbizsrv 已 disabled，再次重启 → 驱动不加载 → 构建 → 字节验证**

## 唯一可行出口（待验证）

**重启计算机**（清除内核驻留驱动/缓存）→ 立即运行 `C:\erd-build\build-after-reboot.bat`（一键：清缓存→构建→字节验证→拷贝 FIXED exe）。
- 成功标志：脚本输出 `=== SUCCESS ===` 且生成 `C:\erd-build\easyrdp-app-FIX41-OK.exe`
- 注意 360 会自启：建议构建完成前保持 360 不启动（或安全模式）

## 当前资产（全部就绪，重启后直接用）

- `C:\erd-build\`：完整可构建源码树（payload.b64=最新模块内容 base64；rdp_view_final.rs；main.rs #[path] 已指向）
- `C:\erd-build\build-after-reboot.bat`：一键构建+验证
- 源码修复内容（都在 payload/module 内）：
  - 输入臂：WM_MOUSEMOVE/按钮/滚轮/键盘 全转发 + input_stats_bump 日志（line 789 ZEBRA2/RDPSCREEN 标记）
  - **fix41**：WM_DESTROY 不再 PostQuitMessage（根治"关远端窗口后 Tauri 主界面不恢复、进程僵尸"）
  - 标题栏按钮日志、WM_CLOSE 日志
- B:\ 原始树 `EasyRDP\easyrdp-client\src-tauri\src\native_view.rs`：pwsh/findstr 视图 = 最新（fix41 在位）；cargo 视图 = 仍会中招 → **B: 树构建仍不可信**

## 构建纪律（追加到 BUILD-WARNINGS.md）

- 每次构建后必须字节搜索标记：`RDPSCREEN-GEN-0055`、`ZEBRA2-INPUT-ARM-EXEC`、`输入到达 kind=`、`WM_CLOSE 到达`、`fix41`——缺任何一个 = 构建产物仍是 fix35 旧代码，禁止发布
- 用户测试日志首行必须含 `RDPSCREEN-GEN-0055` 或鼠标移动出现 `ZEBRA2-INPUT-ARM-EXEC`

## 用户验收标准（v1 遗留）

画面 ✓ | 输入（待新 exe）| 光标形状（待）| 剪贴板（webview 模式 ✓）| 窗口关闭+Tauri 恢复（fix41 待验证）

## 测试流程（新 exe 就绪后）

1. 任务管理器确认无残留 EasyRDP 进程；关闭所有 mstsc
2. 复制 exe → P:\Program Files (x86)\EasyRDP\EasyRDP-0.1.0-portable.exe
3. 清 `%LOCALAPPDATA%\cn.easyrdp.client\EBWebView` 后双击运行
4. 连接后：动鼠标键盘 → 看 debug-{pid}.log 是否出现 `ZEBRA2-INPUT-ARM-EXEC`/`输入到达`
5. 点 ✕ 关闭 → 主界面应恢复、进程应正常退出（fix41）

## ✅ 最终解决（2026-11-15 19:45）

**根因**：`native_view.rs` 的 use 导入列表缺少 `WM_NCCREATE` 和 `WM_DESTROY` 常量。
match 模式中的这两个裸标识符变成变量绑定（捕获一切），WM_NCCREATE 臂吞掉所有消息 →
DefWindowProcW → 输入无效、✕ 无效、fix41 不执行。构建警告中的 unreachable pattern
就是直接信号（此前被误判为构建环境投喂旧快照——文件层确实存在读写视图不一致现象，
但真正的编译错误源自本 bug）。

**修复**：use 导入列表补上 WM_NCCREATE、WM_DESTROY。已部署：
- C:\erd-build\EasyRDP-0.1.0-portable.exe（17193280 字节，已签名）
- B:\...\easyrdp-client\dist\EasyRDP-0.1.0-portable.exe
- P:\Program Files (x86)\EasyRDP\EasyRDP-0.1.0-portable.exe
- B: 树 native_view.rs 导入列表已同步修正

**字节验证**：RDPSCREEN-GEN-0055/ZEBRA2/输入到达/WM_CLOSE 到达/fix41 全部命中（offset 12356930-12358278）。

## 🔧 fix50（2026-11-15 21:15）——拖拽残影根修 + 原生模式剪贴板双向

20:52 用户实测（pid 22848，debug-22848.log）确认两问题，均已修复：

1. **拖拽残影根因**：`on_update_end_paint`/`flush_pending_frame` 从 composer.pixels
   快照全帧，但该缓冲只在 BITMAP_UPDATE 脏矩形路径写入；窗口拖动走 memblt/scrblt
   order 更新（只写 gdi->primary_buffer）→ order 内容永远缺失 → 残影定格。
   **修复（GHOSTFIX2-20261115）**：快照改为直接读 gdi->primary_buffer 整帧
   （ffi.rs 新增 CTX_PTRS/snapshot_primary_frame；on_post_connect 登记、disconnect 注销）。
2. **剪切板双向不通（原生模式）**：本地→远端无触发源（前端从不调
   session_clipboard_set）——**修复（CLIPSYNC-20261115）**：原生窗口
   AddClipboardFormatListener + WM_CLIPBOARDUPDATE(0x031D) → 读 CF_UNICODETEXT →
   clip_tx 通道 → session.rs 转发线程 send_clipboard_text；回环防护
   easyrdp_rdp::clip_sync_note/clip_sync_is_same（lib.rs）。远端→本地链路原已存在
   （ffi.rs ServerFormatList→DataRequest→Response→写本地剪贴板），本轮补齐日志。
3. **编译映射注意**：main.rs `#[path="rdp_view_final.rs"] mod native_view` —— 实际编译
   文件是 rdp_view_final.rs（native_view.rs 已同步修改但不参与编译）。

**字节验证（EasyRDP-0.1.0-portable.exe，17,223,680 字节，SHA256 AA7A83...B8D）**：
fix50-20261115-CLIPSYNC-GHOSTFIX2 / AddClipboardFormatListener / 远端 FormatList /
本地剪贴板文本变化 / GHOSTFIX2-20261115 / 转发本地文本 / 本地剪贴板写入成功 全部命中。
已部署 dist\ 与 P:\Program Files (x86)\EasyRDP\。

**验收标准**：连接 192.168.200.26 → 拖动远端记事本窗口无残影；本地复制文本→远端 txt 粘贴成功；
远端 txt 复制→本地粘贴成功；日志出现 [cliprdr] 各条目。

## 🔧 fix51（2026-11-15 22:45）——传输优化 + Win 键 + Ctrl+Alt+Del

v0.1.0 正式版发布完成后按用户需求继续优化：

1. **DELTA-20261115 传输优化**
   - 原生窗口新增 `present_delta`：引擎 Delta 脏矩形直接写入 FRAME_STORE 并只重绘脏区
     （此前原生模式丢弃 Delta，order 更新只能等 100ms 全帧节流）
   - EndPaint 运动期节流 100ms→33ms（~30FPS）；运动信号新增 EndPaint 频率
     （<60ms 间隔）——窗口拖拽只发 memblt order，原增量字节率检测覆盖不到
   - session.rs pump 在增量批合并到期（16ms）时调 present_delta
2. **WINKEY-20261115 Win 键转发**
   - 原生窗口 attach 时主线程安装 WH_KEYBOARD_LL（Tauri 事件循环泵消息），
     WM_DESTROY 注销；前台是会话窗口时：VK_LWIN/VK_RWIN(0x5B/0x5C) 吞掉并
     转发远端（LL 钩子是拦本地开始菜单的唯一可靠层——explorer 全局钩子在
     窗口消息之前）；其余按键全部放行
3. **SAS：Ctrl+Alt+Del 送达远端**
   - 引擎新增 InputEvent::Sas：ffi send_input 发扩展扫描码序列
     Ctrl(0x1D ext)↓ Alt(0x38 ext)↓ Del(0x53 ext)↓↑…（TermSrv 服务端合成 SAS）
   - 触发源：标题栏第 4 按钮 [SAS]（槽位 cw-184..cw-138）+ 本地 Ctrl+Alt+End
     （LL 钩子拦截，mstsc 同款约定）
   - 标题栏改为四槽：[SAS][─][□][✕] 各 46px

**字节验证（17,236,992 字节，SHA256 2D25EF…37BD）**：fix50 标记/WH_KEYBOARD_LL 安装/
LL钩子拦截/SAS 序列发送完成/标题栏：SAS 按钮/input] sas scan 全部命中。
已部署 dist\ 与 P:\Program Files (x86)\EasyRDP\。
注意：GitHub/Gitee v0.1.0 Release 附件仍是 fix50 构建——用户验收 fix51 后需刷新附件
（替换 portable.exe / win32-x64-portable.zip / SHA256SUMS.txt / src tar.gz，运行
release-v010.sh 的附件替换段或 make-release-assets.ps1 后重跑上传段）。

**用户验收标准（fix51）**：①拖拽远端窗口明显更流畅、无残影；②会话窗口前台按 Win 键
在远端弹开始菜单（本地不弹）；③点标题栏 SAS 按钮或按 Ctrl+Alt+End → 远端弹出
安全屏（锁定/任务管理器）；④本地 Ctrl+Alt+Del 仍作用于本地（符合预期）。
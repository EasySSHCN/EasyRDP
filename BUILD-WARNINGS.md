# ⚠️ EasyRDP 构建警示手册（每次构建必读）

> 本文件记录构建过程中发现的真实坑（2026-11-14 输入无效排查战役实证）。
> **每次构建前必须执行本文件的检查清单，构建后必须执行验证清单。**

## 🔴 坑5（2026-11-15 最终实锤）：WM_* 常量未导入 → match 模式变成"捕获一切"的变量绑定

**真正的根因（此前"默认臂在前"的诊断是误诊）**：

`src-tauri\src\native_view.rs` 的 match msg 分派中，`WM_NCCREATE` 和 `WM_DESTROY` 两个常量
**没有被加入 `use windows_sys::Win32::UI::WindowsAndMessaging::{...}` 导入列表**。

Rust 规则：match 模式中的裸标识符若不是常量，会变成**变量绑定（匹配一切值）**。
于是 `WM_NCCREATE => {...}` 臂变成捕获所有消息的臂 → 所有消息（鼠标移动/点击/键盘/
关闭点击）全部落入该臂 → 只执行 DefWindowProcW → 返回 → 输入完全无效、✕ 无法关闭、
WM_DESTROY 臂永不执行。

**为什么难以发现**：
- 编译只有 warning（unreachable pattern / unused variable），无 error
- 画面显示/光标/探针/attach 都正常（它们在捕获臂之前或不在 match 里）
- 60+ 次构建、3 个工具链版本、2 台机器全部复现——因为 bug 在源码里跟着源码走

**修复（2026-11-15）**：在 use 导入列表补上 `WM_NCCREATE, WM_DESTROY`。

**纪律**：
1. match 新增臂时，必须确认对应常量已在 use 导入列表中
2. 构建警告中的 "unreachable pattern" 和 "unused variable X, did you mean _X"
   涉及 match 模式时 = 立即排查，它们就是这种 bug 的直接信号
3. 构建后必须字节验证新臂的诊断字符串（见坑1验证清单）

## 🔴 坑 1：cargo 增量缓存陈旧（最高优先级）

**症状**：源码已修改，但构建出的 exe 仍包含旧代码。`cargo build` 显示 Finished 但行为不变。
**实锤证据**：字节搜索验证——源码中的字符串字面量（如"输入到达"）在新构建的 exe 中不存在，而旧字符串（如"WM_NCCREATE → DefWindowProc"）仍在。
**根因**：`cargo clean -p <pkg>` 只删最终产物，增量编译缓存（.fingerprint/deps）判定"fresh"后**重链接旧的目标文件**。

**✅ 构建前必须执行**：
```powershell
Remove-Item "$root\target\release\.fingerprint" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "$root\target\release\deps" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "$root\target\release\build" -Recurse -Force -ErrorAction SilentlyContinue
```

**✅ 构建后必须验证（字节搜索，用 UTF8→Latin1 技巧）**：
```powershell
$bytes = [System.IO.File]::ReadAllBytes("$root\target\release\easyrdp-app.exe")
$text = [System.Text.Encoding]::GetEncoding("Latin1").GetString($bytes)
$needle = [System.Text.Encoding]::UTF8.GetBytes("<本次构建的标志性新字符串>")
if (-not $text.Contains([System.Text.Encoding]::GetEncoding("Latin1").GetString($needle))) {
    throw "❌ 构建产物未包含最新源码标记——构建缓存污染！"
}
```
（注意：不能用 ASCII 编码搜索中文——中文是 UTF-8 存储的。）

## 🔴 坑 2：360 文件防护回滚源码修改

**症状**：脚本/pwsh 修改源文件成功（DSH 层可读），但随后真实文件被 360 自动回滚到旧版本。**修改静默丢失。**
**实锤证据**：`[System.IO.File]::WriteAllText` 写入 ZEBRA 标记后，cmd findstr（真实进程）读不到；且"输入到达"日志代码从真实文件中消失。
**适用范围**：360 白名单必须覆盖**整个开发工作区**（含 src 源码目录），仅加 dist 目录不够。

**✅ 处理**：
1. 360 信任区添加：`B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP`（整个目录）
2. 构建前用真实进程验证关键修改已落盘：
```powershell
cmd /c "findstr /C:""<关键新字符串>"" ""<源文件路径>"""
```
3. 若被回滚：临时退出 360 → 重新写入 → 立即构建。

## 🔴 坑 3：debug.log 多线程/多进程交错（已根治，勿回退）

**症状**：日志出现两个时间戳拼接的行、行序与时间戳序矛盾、旧会话日志混入新会话。
**根因**：debug_log 多线程并发写无锁 + 多实例共存并发 append。
**已实施（勿删）**：DEBUG_LOGGER Mutex 串行化 + 命名互斥体跨进程锁 + debug-{pid}.log 按 PID 命名 + 单实例守卫（CreateMutexW）+ 每行 [进程名/PID] 前缀。
**验证**：日志时间戳应单调递增、无拼接行。

## 🔴 坑 4：输入统计计数器进程级累积（测量假象）

**症状**：后续会话的"首条输入到达"日志不出现 = 误判输入无效（实则一直在到达）。
**根因**：INPUT_SEEN/INPUT_COUNTS 进程级静态、从不重置；is_first 只在进程首次为真。
**已实施（勿删）**：attach 时按会话重置计数 + 每类首条即记 + 前 20 条逐条 + 每 100 条直记。
**判读纪律**：读输入日志必须先确认构建标记行（`构建=fixXX-日期`）鉴别会话所属构建。

## 🔴 坑 5：WNDPROC match 臂顺序（历史事故，重构时注意）

**症状（实证）**：所有消息（含 WM_MOUSEMOVE）落入默认臂（日志误标 WM_NCCREATE）→ DefWindowProc 返回 → 输入臂不可达 → 鼠标键盘全灭。
**纪律**：重构 wnd_proc 的 match 时，**输入臂必须在默认臂之前**；默认臂日志文案必须带实际 msg 值（不得写死消息名）。
**验证单测**：wnd_proc 直调 WM_MOUSEMOVE → 断言 INPUT_TX 收到事件（勿删）。

## ✅ 标准构建流程（一键）

```powershell
powershell -ExecutionPolicy Bypass -File "$root\scripts\package-release.ps1"
```
（脚本已内置：缓存清理 → 构建 → 标记验证 → 签名 → 打包。见 package-release.ps1 的"构建警示手册检查"段落。）

## ✅ 构建产物验证（发给用户前必做）

1. 字节搜索本次构建的标志性新字符串（坑 1 的验证代码）
2. 确认 exe 中的构建标记与日志 `构建=` 行可对应
3. 提醒用户：**退出所有旧 EasyRDP 进程**（旧实例残留 = 世代混淆）+ 关闭所有 mstsc 窗口
4. 用户提供 debug-{pid}.log 时：首行构建标记必须匹配本次构建版本

# EasyRDP 方案：macOS / Linux 三平台构建与 CI 发布

> 版本：v1.0（2026-11-16）｜状态：**待用户审批**（批准后实施）
> 前置事实：官网 download.html 的 macOS/Linux 行已改为「尚未提供（规划中）」勘误（R-20261116-008）

## 一、现状盘点（代码事实）

| 层 | 现状 | 跨平台结论 |
|---|---|---|
| 引擎层 `crates/easyrdp-rdp`（FreeRDP FFI） | `native` feature 门控 `freerdp2-sys`；Win32 依赖仅剪贴板（已 cfg(windows) 隔离） | ✅ 天然跨平台：macOS `brew install freerdp`、Linux `apt install libfreerdp2-dev` 即可编译 |
| 呈现层 `src-tauri/src/native_view.rs`（rdp_view_final.rs） | 纯 Win32（HWND/WM_*/GDI 背板/SAS/低级键盘钩子） | ❌ Windows 专属（已 cfg(windows) 隔离） |
| 帧通道 `session.rs` | 二进制 Channel 推 `Framebuffer`（JPEG q75 base64→前端 canvas drawImage）+ `NativeFrame`（raw BGRA→Win32 直绘）双轨 | ✅ JPEG/webview 路径本身跨平台（性能低于原生直绘，但可用） |
| 凭据存储 | Windows DPAPI 加密连接密码 | ❌ 需平台替代 |
| 打包 | `package-release.ps1`：vcpkg 静态 FreeRDP + 单 exe；`make-release-assets.ps1` 五件套 | ❌ 仅 Windows |
| CI | 无（全本机构建） | ❌ 需新建 GitHub Actions |

## 二、两阶段策略

### Phase 1（本方案主体，MVP 可用版）
非 Windows 平台用 **webview canvas 呈现**（复用现有 JPEG 帧管线，零新呈现层开发）：

- 前端 `rdp-view.js` 已有 canvas drawImage 渲染；原生窗口仅在 Windows 启用
- 体验预期：日常办公可用（~30FPS、运动期半分辨率降载已具备）；重度图形/全屏游戏体验不及 Windows 原生直绘（Phase 2 优化）

### Phase 2（后续独立需求）
- macOS：NSWindow + Metal/CALayer 直绘；Linux：X11（x11rb）/Wayland 呈现
- 键盘钩子（Win 键转发）、SAS、剪贴板双向同步按平台 API 分别实现
- 不在本方案范围内，单独立项

## 三、Phase 1 改造清单

### 3.1 客户端代码（预计 2-3 天）
| 项 | 内容 |
|---|---|
| 凭据存储跨平台 | 非 Windows：AES-256-GCM 本地密钥文件（`~/.easyrdp/secret.key`，权限 600）替代 DPAPI——统一接口 `encrypt_secret/decrypt_secret`，Windows 走 DPAPI，其他走密钥文件（同目录绑定 + 首次生成随机密钥） |
| 系统信息采集 | `collect_windows_info` 已 cfg 隔离；补 macOS/Linux 分支（`uname -a`、`sysctl`/`/proc/meminfo`） |
| 反馈 nonce | BCryptGenRandom（Windows）/ /dev/urandom（unix）已具备 ✅ |
| 前端呈现适配 | 非 Windows 下会话直接走 webview canvas（现有逻辑）；`cfg(windows)` 才尝试原生窗口——已如此，仅需验证 |
| 剪贴板 | 非 Windows Phase 1 禁用（UI 隐藏入口），Phase 2 实现（`pbpaste`/`xclip`/`wl-copy`） |
| Win 键/SAS | 非 Windows 无标题栏按钮（标题栏本身是 Win32 自绘）——非 Windows 用 Tauri 原生窗口装饰（decorations:true），后续再对齐自定义标题栏 |

### 3.2 构建/打包（预计 2 天）
| 平台 | 依赖 | 产物 | 打包方式 |
|---|---|---|---|
| macOS (aarch64 + x86_64 Universal) | `brew install freerdp pkg-config` | `EasyRDP.app` → dmg | `cargo tauri build`（tauri自带 dmg 打包；FreeRDP dylib 用 `install_name_tool`/`dylibbundler` 收进 .app） |
| Linux x86_64 | `apt install libfreerdp2-dev libssl-dev` | AppImage | `cargo tauri build --bundles appimage`（linuxdeploy 自动收 so） |
| Windows | 现有 vcpkg 流程不变 | portable exe | 现有 `package-release.ps1` |

- 构建脚本：新增 `scripts/package-release.sh`（mac/linux 通用，对标 ps1：build → stage → dmg/AppImage → 资产命名 `EasyRDP-<ver>-macos.dmg` / `-linux.AppImage`）
- `make-release-assets.ps1` 扩展：检测三平台产物并入 SHA256SUMS 与五件→八件套

### 3.3 CI（GitHub Actions，预计 1 天）

`.github/workflows/release.yml`（tag `v*` 触发）：

```
jobs:
  windows:  windows-latest  → cargo build（vcpkg 缓存）→ portable.exe
  macos:    macos-latest    → 两次构建(aarch64/x86_64) → lipo 合并 Universal → dmg
  linux:    ubuntu-22.04    → cargo build → AppImage
  release:  汇总产物 → SHA256SUMS → GitHub Release 上传 → 触发官网同步
```

- 缓存：cargo-cache + vcpkg 缓存（Windows）
- 签名：macOS ad-hoc 签名（正式 Apple 开发者签名后续接入，当前沿用 SmartScreen/Gatekeeper 警告策略，FAQ 已有说明）
- 网关（easyrdp-gateway）：axum/tokio 纯 Rust ✅ 三平台可构建，Phase 1 暂只发 Windows zip（现状不变）

### 3.4 官网/发布流程联动
- release.sh 第 8 步校验清单扩展：macOS/Linux 行仅在产物存在时显示「本站下载」，否则显示「规划中」（当前勘误状态自动化）
- CI 就绪后移除「尚未提供」标注（回 v0.1.0 时期语义但带真实资产）

## 四、验收标准

- GitHub Actions 三平台 tag 构建全绿，Release 挂齐：`portable.exe` / `win32-x64-portable.zip` / `Gateway zip` / `macos.dmg` / `linux.AppImage` / `src tar.gz` / `SHA256SUMS`（七件）
- macOS 实机（Apple Silicon + Intel 任一）：安装 dmg → 连接 Windows 主机 → 画面/输入可用（JPEG 帧路径）、反馈可提交、多开不崩
- Linux 实机（X11）：同上
- Windows 回归：现有功能无损（原生直绘路径不变）
- 官网下载页三平台状态与实际资产严格一致

## 五、风险与边界

| 风险 | 对策 |
|---|---|
| macOS 上 freerdp2-sys 链接问题（brew keg 路径/符号） | CI 里 pkg-config 探针先行；备选 Homebrew 指定 freerdp@2 版本 |
| JPEG 帧路径性能不及原生（高运动场景卡顿） | Phase 1 预期管理（官网下载页注明）；Phase 2 原生呈现优化 |
| tauri dmg/AppImage 打包细节（图标/权限/linuxdeploy 网络） | CI 内缓存 linuxdeploy；本机先手动试跑一轮 |
| Gatekeeper/SmartScreen 警告 | FAQ 已有说明；正式签名列为后续需求 |
| 非 Windows 密码存储安全强度低于 DPAPI | 密钥文件权限 600 + 绑定本机；文档明示；后续可接 macOS Keychain / libsecret |

## 六、工作量与节奏

- 合计约 5-6 个工作日（代码 2-3 + CI/打包 2-3 + 实机验收 1）
- 批准后顺序：3.1 客户端改造 → 本机 mac/linux 构建试跑（无 mac 硬件则先 CI 出产物实测）→ 3.3 CI → 官网联动 → 随 v0.1.x 版本发布

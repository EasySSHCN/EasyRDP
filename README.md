# EasyRDP

跨平台开源远程桌面客户端（Windows / macOS / Linux）· Rust + Tauri 2 + FreeRDP · MIT 协议

**官方网站：https://EasyRDP.CN** · [GitHub](https://github.com/EasySSHCN/EasyRDP) · [Gitee](https://gitee.com/easyssh/EasyRDP)

## 目录结构

| 目录 | 说明 |
|---|---|
| `easyrdp-client/` | 跨平台客户端（Rust workspace：easyrdp-core + src-tauri 壳） |
| `easyrdp-gateway/` | 云网关服务（axum + tokio：认证/隧道/审计/策略） |
| `easyrdp-site/` | 官网静态站（已部署 https://easyrdp.cn ） |
| `docs/` | 产品 PRD 与架构文档 |

## 快速开始

```bash
# 客户端（需 Rust stable 工具链）
cd easyrdp-client
cargo test -p easyrdp-core     # 核心库单元测试
cargo tauri dev                # 桌面端调试运行

# 网关
cd easyrdp-gateway
cargo test                     # 8 个单元测试
cargo run                      # 默认监听 0.0.0.0:8443
```

## 版本更新历史记录

（本 README 为版本历史权威源，CHANGELOG.md 由本节生成）

### v0.1.0（正式版，2026-11-15）

#### 新增
- **原生 FreeRDP 会话窗口**（Windows）：独立顶层窗口 + 自绘标题栏（最小化/最大化/关闭/拖动），StretchDIBits 直绘全帧，零 JPEG/base64/IPC 开销；Tauri 窗口会话期让位，关闭后自动恢复
- **输入直通**：鼠标（移动/左中右键/滚轮）、键盘全键转发引擎（RDP 坐标系直发）；远端光标形状同步（单色/彩色/32bpp alpha 抗锯齿，AND/XOR 掩码合成）
- **剪贴板双向同步（CLIPRDR CF_UNICODETEXT）**：本地复制 → 远端粘贴、远端复制 → 本地粘贴全链路打通；原生窗口 WM_CLIPBOARDUPDATE 监听 + 双向回环防护（CLIPSYNC）
- **增量帧管线**：BITMAP_UPDATE 脏矩形 → 增量直发 + EndPaint 全帧（100ms 节流 + 5s 兜底 + pending 补发）；运动期自适应降载（半分辨率 + q50，静止 2s 恢复）
- 灾备包机制：源码 + vcpkg FreeRDP 组件 + 恢复文档一键打包（make-dr-package.ps1 + RESTORE.md）

#### 修复
- **拖拽残影（根修）**：全帧快照改从 gdi->primary_buffer 直读——order 更新（memblt/scrblt）此前不进合成缓冲，拖窗口/编辑文本内容永久缺失（GHOSTFIX2）
- **输入失效根因**：WndProc use 导入缺失 WM_NCCREATE/WM_DESTROY 使 match 臂退化为变量绑定吞掉全部消息
- 窗口关闭后 Tauri 主界面恢复/进程退出（WM_DESTROY 不再 PostQuitMessage）
- 移动/缩放黑块残影（WM_ERASEBKGND 返回 1 + WM_MOVE/WM_SIZE InvalidateRect 全帧重绘）
- 光标黑色方块（32bpp 指针 XOR alpha 优先合成）；ACTIVE 后首个输出包掉线（BeginPaint/EndPaint 成对注册）
- 帧管线多线程发射（JPEG 编码移出 FreeRDP 接收线程，拖拽 1-2FPS 卡顿消除）

#### 工程化
- Windows 本地全静态构建管线（vcpkg x64-windows-static-md FreeRDP 2.11.2 + pkgconf + vendored cliprdr/drdynvc/rdpgfx 通道编译）
- 发布产物策略：Windows portable.exe / win32-x64-portable.zip（绿色免安装，单文件静态链接）

### v0.1.0-alpha (2026-11-11)

- 新增：连接中心 UI 原型与客户端 MVP 骨架（Tauri 2 + Rust workspace）
- 新增：easyrdp-core — SessionProfile 模型（serde），含校验逻辑
- 新增：.rdp 文件导入/导出（UTF-16LE/UTF-8 自动识别，key:type:value 解析，往返一致性测试，共 19 个单测）
- 新增：easyrdp-gateway — OIDC→短期 JWT 认证、WebSocket 隧道骨架（审计打点）、环形缓冲审计日志、重定向策略矩阵（8 个单测）
- 新增：官网 https://EasyRDP.CN 上线（首页/功能/下载/更新日志/文档/FAQ/贡献/协议/安全 9 页）
- 新增：产品 PRD 与 UI 原型（基于 mstsc.exe 逆向分析的架构方案）

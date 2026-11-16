# EasyRDP

Windows 开源远程桌面客户端 · Rust + Tauri 2 + FreeRDP · MIT 协议（macOS / Linux 支持规划中）

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

### v0.1.5（2026-11-16）

#### Fixed 修复
- **远程桌面残影收尾修复（撕裂快照免疫）**：快照一致性校验——全帧快照前后对覆盖全帧的 17 个采样行哈希比对，快照期间画面缓冲有写入（撕裂）即丢弃本次快照等下次重试（微秒级开销）；tile 写入后 15ms 内推迟快照。真机取证验证：修复前载入期必捕获 7-8 张撕裂帧 → 修复后全场景 230 帧撕裂帧归零。覆盖 v0.1.4 后仍复现的「载入过程」与「拖动窗口」两场景（鼠标点击残影已在 v0.1.4 根除）

### v0.1.4（2026-11-16）

#### Fixed 修复
- **远程桌面残影根因修复（TILEGHOST，审计驱动 7 项）**：取证定位残影帧本体为「增量模式全帧兜底误用残缺合成缓冲」+「primary_buffer 快照与 tile 合成的撕裂」等 7 处呈现/发射链缺陷：① 兜底全帧改为直读 primary_buffer 完整画面（快照与 tile 写入加互斥锁消除半更新撕裂帧）；② WM_PAINT 不再把旧帧回写背板（背板只读优先，防画面倒退闪现）；③ WM_MOVE 不再整窗失效重绘；④ 增量批双维度守卫（面积>60% 或大量小 tile 风暴整批丢弃等全帧）+ 批像素预校验；⑤ 全帧兜底周期 5s→500ms（进程内 memcpy 零网络代价）；⑥ 运动统计双重累加修正

### v0.1.3（2026-11-16）

#### Fixed 修复
- **远程桌面残影根修（TILEGHOST）**：载入/点击/输入法/窗口拖动时整屏 64×64 tile 网格平铺残影与旧文字叠加——根因定位为服务端位图缓存引用（memblt order 引用失同步的缓存条目被合成到会话缓冲），非呈现层问题。禁用位图缓存协商（BitmapCache/BitmapCacheV3），服务端改发直接位图数据，带宽略增换画面正确性

### v0.1.2（2026-11-16）

#### Fixed 修复
- **使用反馈提交报「no TLS backend is configured」**：ureq 2.x 的 native-tls feature 仅编译后端，快捷方法始终走未编译的 rustls 默认——改为 `AgentBuilder::new().tls_connector(native_tls::TlsConnector)` 显式挂载 schannel 后端，Agent 全局复用（OnceLock）减少连接新建；网络类失败自动重试一次并在失败文案给出可操作建议
- **主界面标题栏版本号仍显示 v0.1.0**：tag 由硬编码改为动态读取应用版本（`app.getVersion()`），随 tauri.conf 版本自动更新

### v0.1.1（2026-11-16）

#### 新增
- **用户反馈功能**：标题栏「反馈」按钮，支持功能建议/问题反馈/其他分类、联系方式选填、5-2000 字内容（带字数校验）；反馈经 AES-256-GCM 加密信封上传官网（HMAC-SHA256 派生密钥 + 时间窗防重放），随反馈匿名采集系统基础信息（版本/系统/分辨率，不含任何个人身份信息）用于问题定位
- **官网「用户之声·反馈精选」**：下载页新增板块，展示已回复且公开的用户反馈与官方回复（用户可在后台自主控制是否公开）
- **官网统一后台**：管理员登录（图形验证码 + 防爆破锁定），反馈管理支持列表筛选/回复/公开/删除/补录

#### 修复
- **原生远程桌面窗口点击闪现错位/拼贴画面**（fix36）：新增每窗口 GDI 背板（内存 DC + 兼容位图），帧经 SetDIBitsToDevice 整帧入背板后统一 StretchBlt 呈现，消除 top-down DIB 子矩形源 y 坐标的驱动级歧义；半帧判别收紧为精确等长，异常长度缓冲丢弃等全帧兜底；窗口销毁释放背板防泄漏

#### 官网/基础设施
- 反馈 API（提交/管理/公开 TOP10/图形验证码）上线；每日自动采集用户反馈入需求池 + AI 复审（real 入池公开 / spam 下架 / reject 关闭回复）闭环

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

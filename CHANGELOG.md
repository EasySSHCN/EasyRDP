# Changelog

由 README.md「版本更新历史记录」小节生成，README 为权威源。

## v0.1.0 (2026-11-15)

### Added
- 原生 FreeRDP 会话窗口（Windows）：独立顶层窗口 + 自绘标题栏，StretchDIBits 直绘全帧；Tauri 窗口会话期让位
- 输入直通：鼠标（移动/左中右键/滚轮）、键盘全键转发；远端光标形状同步（含 32bpp alpha）
- 剪贴板双向同步（CLIPRDR CF_UNICODETEXT）：本地↔远端复制粘贴全链路 + 回环防护（CLIPSYNC）
- 增量帧管线：脏矩形增量直发 + EndPaint 全帧节流/兜底/pending 补发；运动期自适应降载
- 灾备包机制：源码 + vcpkg 组件 + 恢复文档一键打包

### Changed
- 版本号 0.1.0-alpha → 0.1.0；Windows 便携版发布策略（portable.exe / win32-x64-portable.zip）
- Windows 本地全静态构建管线（vcpkg static-md FreeRDP 2.11.2 + vendored cliprdr/drdynvc/rdpgfx）

### Fixed
- 拖拽残影根修：全帧快照改读 gdi->primary_buffer（order 更新内容缺失根因，GHOSTFIX2）
- 输入失效根因：WndProc use 缺失 WM_NCCREATE/WM_DESTROY 臂退化吞消息
- 关窗后主界面恢复/进程退出；移动缩放黑块残影；光标黑方块；ACTIVE 后首包掉线；拖拽 1-2FPS 卡顿

## v0.1.0-alpha (2026-11-11)

### Added
- 连接中心 UI 原型与客户端 MVP 骨架（Tauri 2 + Rust workspace）
- easyrdp-core：SessionProfile 模型（serde），含校验逻辑
- .rdp 文件导入/导出（UTF-16LE/UTF-8 自动识别，往返一致性测试，共 19 个单测）
- easyrdp-gateway：短期 JWT 认证、WebSocket 隧道骨架、审计日志、重定向策略矩阵（8 个单测）
- 官网 https://EasyRDP.CN 上线（9 个页面）
- 产品 PRD 与 UI 原型

### Changed
- 无（首个版本）

### Fixed
- 无（首个版本）

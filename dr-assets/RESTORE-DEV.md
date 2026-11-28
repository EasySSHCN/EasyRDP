# 开发机角色恢复指南（RESTORE-DEV.md）

原开发机：Windows 10/11（项目 B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP，
依赖 B:\tools\EasyRDP；DSH 安装于 P:\Program Files (x86)\DeepSeek Harness）

## Windows 开发机（主力构建/发布）

```powershell
# 管理员 PowerShell，cd 到本脚本目录
powershell -ExecutionPolicy Bypass -File install-windows.ps1
# 可选参数：-ProjectDir ... -ToolsDir ... -SkipDSH -SkipWsl -SkipScheduledTasks
```

覆盖：项目源码部署（EasyRDP 源码目录 robocopy 增量）、构建依赖工具链（vcpkg/FreeRDP 快照）、
git/Node/Rust/VS2022 BuildTools/WebView2/WSL2、
DSH 开发角色（已装自动识别；winget 无收录则按脚本提示手动装）、
定时任务（反馈采集 08:00/20:00、发布巡检 04:00）、构建环境变量样板
easyrdp-build-env.ps1。

## Linux 开发机（辅助/CI）

```bash
sudo bash install-linux.sh --project=/opt/EasyRDP
# 可选：--skip-dsh 跳过 DSH 安装指引
```

覆盖：Rust/Node/pwsh、Tauri Linux 依赖、系统 FreeRDP2 开发库、
路径硬编码修正（B:\ → 安装路径）、crontab 定时任务、DSH 开发角色安装指引。

## 安装后自检

1. `. easyrdp-build-env.ps1`（Windows）后：
   `cargo check -p easyrdp-rdp --features native` 成功；
2. `cargo test --manifest-path src-tauri\Cargo.toml` 43+ 通过；
3. `powershell scripts\release-check.ps1` 巡检可跑（需求池可读）；
4. wsl 内 `bash scripts/release.sh <版本>` 可走通（凭据自动解密）；
5. DSH 打开正常，工作目录指向项目路径，Web GUI 可访问。

## 关键文档（EasyRDP 源码目录内 docs/）

- WINDOWS-FREERDP-SETUP.md（构建环境，必读）
- RELEASE.md（发布流程 8 步 + OTA 第 9 步手动补跑方法）
- RESTORE.md（本灾备体系总指南）
- 方案-在线更新机制.md / HANDOFF-NATIVE-INPUT.md

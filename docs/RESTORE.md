# EasyRDP 灾备包恢复指南（RESTORE.md）

打包时间：2026-11-15 23:0x（源码版本：fix51——DELTA/WINKEY/SAS；发布流程 v2 见 docs/RELEASE.md）
适用场景：异地设备（**不依赖 DeepSeek Harness**，任何 Windows 10/11 + Rust 工具链均可）恢复本项目当前开发进程。

## 包内容

```
EasyRDP-DR-20261115/
├── RESTORE.md                     ← 本文档
├── src/EasyRDP/                   ← 项目全部源码（排除各 target 构建目录）
│   ├── easyrdp-client/            ← Tauri 2 客户端（Rust workspace：easyrdp-core/-rdp/-app + src-tauri）
│   │   └── dist/                  ← 全部历史发布产物（含 EasyRDP-0.1.0-portable.exe fix50）
│   ├── easyrdp-gateway/           ← 网关（Rust：JWT + WebSocket 隧道）
│   ├── easyrdp-site/              ← 官网 https://EasyRDP.CN
│   ├── docs/                      ← 架构/RDPEGFX/FreeRDP 环境文档（必读 WINDOWS-FREERDP-SETUP.md）
│   ├── .github/                   ← CI 工作流
│   ├── signing/                   ← 签名材料
│   ├── HANDOFF-NATIVE-INPUT.md    ← 会话交接/踩坑全记录（必读）
│   ├── BUILD-WARNINGS.md          ← 构建纪律（必读）
│   ├── CHANGELOG.md / README.md
│   ├── tools-*.sh                 ← 发布/推仓库脚本（bash + GitHub/Gitee token）
│   └── make-dr-package.ps1        ← 本灾备包制作脚本（可重复打包）
└── tools/EasyRDP/                 ← 构建依赖组件
    ├── vcpkg/                     ← vcpkg 快照（master，含 vcpkg.exe）
    │   ├── installed/             ← x64-windows + x64-windows-static-md 全套库/头文件（FreeRDP 2.11.2）
    │   ├── buildtrees/freerdp/src/2.11.2-88e305a3a0.clean/  ← FreeRDP 源码树（build.rs vendor 编译 cliprdr/drdynvc/rdpgfx 必需）
    │   ├── ports/ + scripts/      ← 端口树与脚本
    │   └── bootstrap-vcpkg.bat
    ├── pkgconf-msi/               ← pkgconf 3.0.10（pkg-config 探测用，MSI 解包版）
    ├── overlay-ports/             ← freerdp@2.11.2 钉扎端口（重装 vcpkg 包时必须）
    ├── freerdp2-sys-0.2.0/ + f2sys.crate  ← Rust 绑定 crate 源码缓存
    ├── baseline.txt / freerdp-versions.json / pkgconf.msi
    └── （vcpkg downloads 缓存 2.5GB 未包含——异地重装时经 ghfast.top 镜像重新下载）
```

## 异地恢复步骤（全新 Windows 设备）

1. **安装基础工具链**
   - Rust 1.99+ MSVC（rustup，工具链 `stable-x86_64-pc-windows-msvc`）
   - Visual Studio 2022（含"使用 C++ 的桌面开发"，提供 cl.exe/链接器）
   - git、PowerShell（脚本均为 pwsh/PowerShell 兼容）

2. **摆放目录**（保持盘符一致性可免改配置；不同盘符见第 6 步）
   ```
   <任意盘>\EasyRDP\            ← src\EasyRDP\ 的内容
   <任意盘>\tools\EasyRDP\      ← tools\EasyRDP\ 的内容
   ```
   本机原路径为 `B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP`
   与 `B:\tools\EasyRDP`。

3. **会话级构建环境变量**（每次构建前设置；见 docs/WINDOWS-FREERDP-SETUP.md）
   ```powershell
   $env:PKG_CONFIG_PATH = '<vcpkg根>\installed\x64-windows-static-md\lib\pkgconfig'
   $env:PATH = '<pkgconf根>\PFiles64\pkgconf-3.0.10;' + $env:PATH
   # 按包内布局：
   #   PKG_CONFIG_PATH = <tools>\EasyRDP\vcpkg\installed\x64-windows-static-md\lib\pkgconfig
   #   PATH 追加        <tools>\EasyRDP\pkgconf-msi\PFiles64\pkgconf-3.0.10
   ```

4. **编译客户端**（FreeRDP/WinPR/OpenSSL 全静态链接，产物单 exe，无 DLL 依赖）
   ```powershell
   cd <EasyRDP>\easyrdp-client
   powershell -ExecutionPolicy Bypass -File scripts\package-release.ps1
   # 或手动：
   cargo build --release -p easyrdp-app --features custom-protocol,native
   # 产物：target\release\easyrdp-app.exe
   ```

5. **构建纪律（BUILD-WARNINGS.md + HANDOFF，务必遵守）**
   - main.rs 的 `#[path = "rdp_view_final.rs"] mod native_view`：**实际编译的视图模块是 rdp_view_final.rs**，改 native_view.rs 不生效
   - 构建后必须字节搜索标记验证：`fix50-20261115-CLIPSYNC-GHOSTFIX2`（缺失 = 构建污染，禁止发布）
   - WndProc 的 match 臂新增常量必须同步加进 use 导入列表（历史上 WM_NCCREATE 缺失曾把输入臂变成变量绑定吞掉全部消息）
   - 运行日志：`%APPDATA%\EasyRDP\debug-{pid}.log`

6. **盘符/路径不一致时的适配点**
   - `crates\easyrdp-rdp\build.rs`：vcpkg 根默认 `B:\tools\EasyRDP\vcpkg`，可用环境变量 `EASYRDP_VCPKG_ROOT` 覆盖
   - `scripts\package-release.ps1`：`$VcpkgRoot` / `$PkgconfRoot` 参数可覆盖
   - cargo registry 镜像（rsproxy.cn）配置在 `%USERPROFILE%\.cargo\config.toml`，异地按需重配

7. **（可选）重装 vcpkg FreeRDP 2.11.2**（仅当 installed 损坏时）
   ```powershell
   $env:X_VCPKG_ASSET_SOURCES = 'x-script,curl.exe -L -sS --fail --output {dst} --create-dirs https://ghfast.top/{url}'
   <vcpkg>\vcpkg.exe install freerdp:x64-windows --overlay-ports=<tools>\EasyRDP\overlay-ports
   ```

8. **运行时**
   - exe 双击即用（全部静态链接；调试日志写 `%APPDATA%\EasyRDP\`）
   - 网关/官网见各自目录内 README
   - **发布**：一键 `bash scripts/release.sh <版本> [--skip-build]`（WSL/Linux，7 步流程见 docs/RELEASE.md）；凭据在 `secrets-vault/`（creds.enc + key 加密保管，解密方法见该目录 README.txt，禁止外泄）

## 9. 发布流程（v2 摘要，详见 docs/RELEASE.md）

- 一键：`bash scripts/release.sh <版本> [附言|--skip-build]`
- 7 步：凭据解密(加密库) → 构建+资产 → SHA256 校验 → git 双平台 push → 说明取 README → GitHub Release(幂等替换附件) → Gitee Release(删除重建, 100MB 上限保护)
- 凭据：secrets-vault/（AES-256-CBC/PBKDF2；env GITHUB_TOKEN/GITEE_TOKEN 可覆盖）
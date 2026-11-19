# EasyRDP 灾备包恢复指南（RESTORE.md）

打包时间：2026-11-17（源码版本：v0.3.3——文件剪贴板双向传输 + 文本剪贴板协议修复；发布流程见 docs/RELEASE.md）
适用场景：异地设备（**不依赖 DeepSeek Harness**，任何 Windows 10/11 + Rust 工具链均可）**完整复刻**本项目全部内容——源码、构建依赖、历史发布产物、需求池流水线、密钥库、官网源与发布管线。

## 包内容

```
EasyRDP-DR-20261117/
├── RESTORE.md                     ← 本文档
├── src/EasyRDP/                   ← 项目全部源码（排除各 target 构建目录；含机密库见下）
│   ├── easyrdp-client/            ← Tauri 2 客户端（Rust workspace：easyrdp-core/-rdp/-app + src-tauri）
│   │   ├── dist/                  ← 全部历史发布产物（EasyRDP-0.1.0 ~ 0.3.3 portable.exe/win64.zip 等）
│   │   ├── scripts/package-release.ps1  ← 客户端打包脚本
│   │   └── crates/easyrdp-rdp/examples/clip_probe.rs  ← 剪贴板专项无头探针（取证工具）
│   ├── easyrdp-gateway/           ← 网关（Rust：JWT + WebSocket 隧道）
│   ├── easyrdp-site/              ← 官网 https://easyrdp.cn 全源（页面/样式/api PHP/更新接口）
│   ├── reqpool/                   ← 需求池双轨数据（机器池 JSON + 人读文档镜像 + 巡检/采集日志）
│   │   ├── requirements-pool.json ← 发布巡检唯一权威数据源
│   │   ├── pending-items/         ← 待登记条目（pool-add.ps1 输入）
│   │   └── release-request.json   ← 巡检生成的发布请求（存在即待发布会话处理）
│   ├── docs/                      ← 架构/环境/发布流程/机制方案文档（必读清单见下）
│   ├── scripts/                   ← 全流程脚本（见下「流水线脚本清单」）
│   ├── secrets-vault/             ← 【机密】加密凭据库（见下「机密清单」）
│   ├── signing/                   ← 【机密】内部开发签名材料
│   ├── .github/                   ← CI 工作流
│   ├── HANDOFF-NATIVE-INPUT.md    ← 会话交接/踩坑全记录（必读）
│   ├── BUILD-WARNINGS.md          ← 构建纪律（必读）
│   ├── CHANGELOG.md / README.md   ← 版本历史（README 为权威源）
│   └── make-dr-package.ps1        ← 本灾备包制作脚本（可重复打包）
└── tools/EasyRDP/                 ← 构建依赖组件（异地构建必需）
    ├── vcpkg/                     ← vcpkg 快照（含 vcpkg.exe）
    │   ├── installed/             ← x64-windows + x64-windows-static-md 全套库/头文件（FreeRDP 2.11.2）
    │   ├── buildtrees/freerdp/src/2.11.2-88e305a3a0.clean/  ← FreeRDP 源码树（build.rs vendor 编译 cliprdr/drdynvc/rdpgfx 必需）
    │   ├── ports/ + scripts/      ← 端口树与脚本
    │   └── bootstrap-vcpkg.bat
    ├── pkgconf-msi/               ← pkgconf 3.0.10（pkg-config 探测用，MSI 解包版）
    ├── overlay-ports/             ← freerdp@2.11.2 钉扎端口（重装 vcpkg 包时必须）
    ├── freerdp2-sys-0.2.0/ + f2sys.crate ← Rust 绑定 crate 源码缓存
    ├── baseline.txt / freerdp-versions.json / pkgconf.msi
    └── （vcpkg downloads 缓存未包含——异地重装时经镜像重新下载）
```

## 机密清单（本包内均为加密形态；明文只应存在于持有者掌握处）

| 文件/项 | 内容 | 用途 |
|---|---|---|
| `src/EasyRDP/secrets-vault/creds.enc` + `key` | GitHub/Gitee API 令牌（AES-256-CBC/PBKDF2 加密，release.sh 自动解密） | 发布 push / Release 创建 |
| `src/EasyRDP/secrets-vault/site-server.enc` + `key` | 官网服务器 host/port/ssh账号/webroot | release.sh 第 8 步官网同步、OTA 上传、后台维护 |
| `src/EasyRDP/secrets-vault/ota-keys.enc` + `key` | OTA Ed25519 签名私钥/公钥/主密钥 | release.sh 第 9 步加密升级包与清单签名 |
| `src/EasyRDP/secrets-vault/ota-pub.b64` | Ed25519 公钥（与 updater.rs 内嵌值一致） | 验签对照 |
| 服务器 `easyrdp.cn:/www/wwwroot/easyrdp.cn/api/feedback/keys.php` | 反馈后台 admin_user/admin_hash(bcrypt)、token_secret、反馈加密 master、邮件网关 | **只在服务器存在**（不在本包）；admin 登录凭据由项目持有者自行保管 |
| SSH 私钥 `/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh`（P 盘，**不在本包**） | 官网服务器 SSH 密钥 | 灾备机如需访问官网服务器，需另行迁移此私钥 |

## 流水线脚本清单（src/EasyRDP/scripts/）

| 脚本 | 用途 |
|---|---|
| `release-check.ps1` | 每日巡检：池内「记录」≥3 → 生成 release-request.json（里程碑规则：池累计每满 10 条升 minor） |
| `pool-add.ps1` | 需求入池唯一通道（校验格式/回读复核） |
| `next-version.ps1` | 版本号计算（-Current 必填；里程碑时加 -Minor） |
| `release.sh` | 一键发布（WSL bash）：构建→脱敏门禁→资产校验→push 双平台→GitHub/Gitee Release→官网同步 12 项校验→OTA（第 9 步） |
| `ota-package.ps1` / `ota-upload.sh` | OTA 清单签名+加密包生成 / 手动补传（release.sh 第 9 步 wslpath 缺陷的替代路径，见 RELEASE.md） |
| `harvest-feedback.ps1` / `apply-review.ps1` | 官网反馈采集入池 / AI 复审回写 |
| `notify-email.ps1` | 发布通知邮件（ad@6xm.net） |
| `verify-ota.mjs` / `probe-check-php.mjs` | OTA 端到端验签探针 |

## 异地恢复步骤（全新 Windows 设备）

1. **安装基础工具链**
   - Rust 1.85+ MSVC（rustup，工具链 `stable-x86_64-pc-windows-msvc`）
   - Visual Studio 2022（含"使用 C++ 的桌面开发"，提供 cl.exe/链接器）
   - git、PowerShell 5.1+、**WSL2（Ubuntu）+ bash**（release.sh 依赖）
   - Node.js 22+（easyrdp-site 前端构建/探针脚本用）

2. **摆放目录**（保持盘符一致性可免改配置；不同盘符见第 6 步）
   ```
   <任意盘>\EasyRDP\            ← src\EasyRDP\ 的内容
   <任意盘>\tools\EasyRDP\      ← tools\EasyRDP\ 的内容
   ```
   本机原路径为 `B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP`
   与 `B:\tools\EasyRDP`。

3. **会话级构建环境变量**（每次构建前设置；见 docs/WINDOWS-FREERDP-SETUP.md）
   ```powershell
   # 按包内布局：
   $env:PKG_CONFIG_PATH = '<tools>\EasyRDP\vcpkg\installed\x64-windows-static-md\lib\pkgconfig'
   $env:PATH = '<tools>\EasyRDP\pkgconf-msi\PFiles64\pkgconf-3.0.10;' + $env:PATH
   ```
   验证：`cargo check -p easyrdp-rdp --features native`（在 easyrdp-client 下执行）应成功。

4. **客户端构建/发布**
   - 构建：`powershell -File easyrdp-client\scripts\package-release.ps1`
   - 完整发布：`wsl bash -c "cd <项目路径> && bash scripts/release.sh <版本号> [附言]"`
     （凭据自动从 secrets-vault 解密；官网同步与 OTA 自动执行）

5. **官网（easyrdp.cn）恢复**（服务器本身另需迁移，本包提供其全部源）
   - webroot `/www/wwwroot/easyrdp.cn` ← `src/EasyRDP/easyrdp-site/`（downloads/ 与 api/feedback/keys.php 除外——后者只在服务器）
   - 上传后 `chown www:www`（nginx 权限要求，否则 404）
   - HTML 已配置 no-cache 协商缓存
   - 需要服务器 SSH 时：私钥见上「机密清单」最后一条（不在本包）

6. **不同盘符/路径**：全局搜索项目内 `B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP`
   与 `B:\tools\EasyRDP` 字面量并替换（集中在 scripts/*.ps1、make-dr-package.ps1、
   docs/WINDOWS-FREERDP-SETUP.md、scripts/release.sh 的 ROOT 变量）。

7. **需求池流水线恢复**
   - 每日巡检：`powershell -ExecutionPolicy Bypass -File scripts\release-check.ps1`
   - 反馈采集（需配 EASYRDP_MAIL_API_* 或本机 EasySSH DPAPI 凭据）：`scripts\harvest-feedback.ps1`
   - 需求入池：写 `reqpool\pending-items\R-<yyyymmdd>-<流水>.json` → `scripts\pool-add.ps1 -ItemFile <该文件>`

## 完整复刻自检清单

- [ ] `cargo check -p easyrdp-rdp --features native` 通过（构建依赖就位）
- [ ] `cargo test --manifest-path easyrdp-client\src-tauri\Cargo.toml` 43+ 通过
- [ ] `powershell -File scripts\release-check.ps1` 巡检可跑（需求池可读）
- [ ] release.sh 第 1 步凭据解密成功（creds.enc 可解）
- [ ] 官网可访问且 main.js 徽章版本与 tauri.conf.json 一致
- [ ] 软件端「检查更新」可正常返回（latest.json 验签通过）

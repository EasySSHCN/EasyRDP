# EasyRDP CI / 发布流程

## 文件

| 文件 | 用途 |
| --- | --- |
| `.github/workflows/ci.yml` | push / PR 到 `main` 时：三平台 `cargo test --workspace` + release 构建自检，不发布产物 |
| `.github/workflows/release.yml` | 推送 `v*` 标签时：三平台构建、签名（可选）、生成校验和、发布 GitHub Release |

## 仓库结构约定

- 仓库根 = `EasyRDP/`（`.github/workflows/` 放在仓库根，GitHub 才能识别）。
- Cargo workspace 根在 `easyrdp-client/`（`crates/easyrdp-core`、`crates/easyrdp-rdp`、`src-tauri` 三个 member）。
- Tauri 工程在 `easyrdp-client/src-tauri/`；版本号以 `src-tauri/tauri.conf.json` 的 `version` 为准（当前 `0.1.0-alpha`）。

## 构建要点

1. **custom-protocol 必须启用**：
   - CI 直接使用 `cargo build --release -p easyrdp-app --features custom-protocol`。
   - Release 使用 `cargo tauri build --features custom-protocol`（tauri 本身也会启用，显式传入双保险）。
   - 不启用时 tauri 以 `dev` 模式编译，运行时会连 `devUrl`（localhost:5173）导致白屏。
2. **tauri-cli 安装**：Release 工作流用 `taiki-e/install-action@tauri-cli` 拉取预编译二进制，比 `cargo install tauri-cli --locked`（需从源码编译，约 5–10 分钟）快得多。
3. **Linux 依赖**：按 Tauri v2 官方清单安装 `libwebkit2gtk-4.1-dev`、`libxdo-dev`、`libssl-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev` 等；AppImage 打包/运行另需 `libfuse2`（ubuntu-22.04）。
4. **产物命名**（`<ver>` 取自 tauri.conf.json）：
   - Windows：`EasyRDP-<ver>-portable.exe`（单文件）、`EasyRDP-<ver>-win32-x64-portable.zip`；如生成 MSI/NSIS 安装器也一并附加。
   - macOS：`EasyRDP-<ver>-macos.dmg`
   - Linux：`EasyRDP-<ver>-linux.AppImage`（可选附 deb/rpm）
   - 汇总后由 `publish` job 生成覆盖**全部产物**的 `SHA256SUMS.txt` 一并附加到 Release。
5. **发布 job**：三个构建 job 各自通过 `actions/upload-artifact@v4` 上传本平台产物；`publish` job 用 `actions/download-artifact@v4`（`merge-multiple: true`）合并、生成校验和后经 `softprops/action-gh-release@v2` 附加到 tag 对应的 Release。tag 含 `alpha`/`beta`/`rc` 时自动标记为 prerelease，并启用自动 Release Notes。

## 发版步骤

```bash
# 1. 确认 easyrdp-client/src-tauri/tauri.conf.json 的 version 已更新
# 2. 提交并打标签（标签必须以 v 开头）
git tag v0.1.0-alpha
git push origin main --tags
# 3. 等待 Actions 三平台构建完成，产物出现在对应 GitHub Release 页
```

## 启用 Azure Trusted Signing（Windows 代码签名，可选）

发布工作流已预留签名步骤（`azure/trusted-signing-action@v0`）。**未配置 secrets 时该步骤自动跳过**，不影响发布。

开通与配置步骤：

1. 开通 Trusted Signing 资源（需 Azure 订阅并完成身份验证；个人开发者 $9.99/月）：
   <https://learn.microsoft.com/azure/trusted-signing/overview>
2. 在 Trusted Signing 账号下创建证书 profile，记下 Account Name、Endpoint URL（如 `https://eus.codesigning.azure.net/`）、Certificate Profile Name、Certificate Name。
3. 创建 Entra ID App Registration，授予该资源 `Trusted Signing Certificate Profile Signer` 角色，取得 Tenant ID / Client ID / Client Secret。
4. 在 GitHub 仓库 **Settings → Secrets and variables → Actions** 配置以下 secrets：

   | Secret | 说明 |
   | --- | --- |
   | `AZURE_SIGNING_ACCOUNT` | Trusted Signing 账号名（非空即触发签名步骤） |
   | `AZURE_SIGNING_ENDPOINT` | 区域 endpoint URL |
   | `AZURE_SIGNING_CERT_PROFILE` | 证书 profile 名 |
   | `AZURE_SIGNING_CERT_NAME` | 证书名 |
   | `AZURE_SIGNING_TENANT_ID` | Entra 租户 ID |
   | `AZURE_SIGNING_CLIENT_ID` | App Registration 客户端 ID |
   | `AZURE_SIGNING_CLIENT_SECRET` | 客户端密钥 |

5. 下一次 tag 发布时，`EasyRDP-<ver>-portable.exe` 及安装器 exe 将被自动签名（SHA256 摘要 + RFC3161 时间戳）。

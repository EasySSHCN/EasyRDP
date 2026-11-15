# EasyRDP 发布流程（v2，2026-11-15）

> 对标 EasySSH `scripts/release.sh` 整理改进；一键入口：`scripts/release.sh`（bash，WSL/Linux 通用）

## 一键发布

```bash
# WSL bash（本机）或任意 Linux：
bash scripts/release.sh 0.1.0                 # 全流程（构建→校验→push→双平台 Release）
bash scripts/release.sh 0.1.0 "修复说明"      # 带附言（追加在 README 版本条目之前）
bash scripts/release.sh 0.1.0 --skip-build    # 跳过构建（资产已就绪时）
```

## 流程（7 步）

| 步骤 | 内容 | 来源借鉴（EasySSH） |
|---|---|---|
| 1 凭据 | 环境变量 `GITHUB_TOKEN/GITEE_TOKEN` 优先 → 否则从项目加密库 `secrets-vault/`（creds.enc + key，AES-256-CBC/PBKDF2）解密；明文只存在于进程变量，不落盘 | gitee-vault 自动解密回退 |
| 2 构建 | powershell 调 `easyrdp-client/scripts/package-release.ps1`（cargo release 静态链接）+ `make-release-assets.ps1`（portable.zip/src tar.gz/SHA256SUMS） | `npm run package:win` |
| 3 校验 | 五件资产存在性 + du 体积 + SHA256SUMS 与 portable.exe 实际哈希比对，缺失即终止 | —（新增，EasySSH 无） |
| 4 push | 解 src tar → git init/commit/tag → GitHub `x-access-token@` + **HTTP/1.1**、Gitee `easyssh:` 双平台 push；Gitee 确认仓库公开 | askpass/vault push |
| 5 说明 | Release 说明取 README `### v$VER` 条目（权威源）+ 可选附言；正文带官网抬头 | awk 提取 + 官网抬头 |
| 6 GitHub | Release 已存在 → 全附件替换（幂等重发）；不存在 → 创建；逐附件回报 state | upload_url 上传 |
| 7 Gitee | 同 tag Release 删除重建（五件全量替换）；token 在查询串（表单字段会挂起）；**单附件 >100MB 自动跳过**；`--max-time 7200` | 同 tag 复用/100MB 上限/max-time |
| 8 官网 | ① **版本徽章同步**（VERBADGE 节点）：`assets/js/main.js` 的 `SITE_VERSION` → 本次版本（所有 `[data-version]` 徽章由它运行时填充，**只改 HTML 不生效**——11-15 事故）；② `main.js?v=N` 缓存穿透（N 取服务器现值 +1 自动递增）；③ 9 页 + JS 上传；④ 线上核验徽章常量与旧版本串残留（changelog 历史条目豁免）；⑤ downloads/ 五件上传 | 官网 sync/deploy 步 |

## 与 EasySSH 流程的差异（EasyRDP 特化）

1. **构建端**：Windows Rust/Tauri 静态链接（单 exe），非 node/electron 打包
2. **资产**：五件套（源码 tar 也作为 Release 资产发布，alpha 起的惯例）；EasySSH 只发 zip
3. **GitHub git 凭据形态**：`x-access-token:<token>` 用户 + `http.version=HTTP/1.1`
   ——本机实测 `EasySSHCN:<token>` 认证失败、HTTP/2 有 curl 16 framing 层错误
4. **Gitee 同 tag 语义**：删除重建（全量替换资产）；EasySSH 是复用追加
5. **校验步**：发布前强制资产完整性检查（EasySSH 无此步）
6. **官网同步（第 8 步）**：easyrdp.cn 托管于独立服务器（信息在 `secrets-vault/site-server.enc`，
   密钥与 la.trae.show 相同；webroot `/www/wwwroot/easyrdp.cn`）——上传 9 页 + downloads/
   五件产物 + SHA256SUMS；发布源码包（官网/GitHub/Gitee 等一切对外渠道）**必须先脱敏**
   （含密钥/服务器 IP/内网拓扑/个人信息的文件一律不得进入发布物）

## 安全事件台账

- **2026-11-15 23:0x 泄漏事件（已处置）**：v0.1.0 发布的源码包（GitHub/Gitee/官网三渠道）
  与 git 历史包含 `secrets-vault/`（creds.enc+key=双平台令牌）与 `signing/`
  （pfx-password.txt 明文口令 + easyrdp-internal.pfx 签名材料）。处置：源码包三渠道全量
  替换为脱敏版（扫描 0 命中）；git 双平台 filter-branch 净化历史 + 强推（v0.1.0 标签重指向）；
  make-release-assets.ps1 / scripts/release.sh 固化脱敏门禁（命中即终止）。
  **待用户轮换**：GitHub 令牌、Gitee 令牌（泄漏窗口内下载过源码包即可解密）；内部开发签名
  证书按已泄漏处理。注意：GitHub 对已不可达的历史提交可能按 SHA 直取保留一段时间，轮换是根本补救。

## 📌 永久规则（不得违反，全文如下，无其他版本）

**永久记住：对外（官网、GitHub、Gitee等）发布源码包时候必须脱敏**

——发布前逐文件检查：加密凭据、服务器 IP/主机名、SSH 密钥、内网拓扑、
口令/令牌、个人信息，任何一项命中即禁止发布。

**凭据保管**：一切服务器/令牌信息只允许以加密形式存于 `secrets-vault/`
（AES-256-CBC/PBKDF2），明文禁止进入任何仓库、发布包、灾备包明文区。

## 凭据保管

- 位置：`secrets-vault/{creds.enc, key}`（隐藏目录 + .gitignore，**禁止外泄/提交**）
- 格式：`creds.enc` = AES-256-CBC(PBKDF2) 密文，内容两行 `github_api_token:` / `gitee_api_token:`；`key` = 64 hex 字符密钥
- 更新令牌：见 `secrets-vault/README.txt`
- 灾备：`make-dr-package.ps1` 会把 secrets-vault 一并打包

## 历史脚本

- `tools-*.sh`（alpha 期 GitHub/Gitee 操作脚本）保留作参考；发布一律走 `scripts/release.sh`

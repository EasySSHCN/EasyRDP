# EasyRDP 发布流程（v2，2026-11-15；v3 2026-11-16 强化逐条校验）

> 对标 EasySSH `scripts/release.sh` 整理改进；一键入口：`scripts/release.sh`（bash，WSL/Linux 通用）

## 📌 版本号三级递进规则（2026-11-16 用户口径，全流程节点必须遵守）

以 v.1.2.3 为例：

1. **一级版本号（major，最高级）**：满足 10 次二级版本号更新才进行升级（minor 满 10 → major+1，minor/patch 归零）
2. **二级版本号（minor，中级）**：满足 10 次三级版本号更新才进行升级（patch 满 10 → minor+1，patch 归零）
3. **三级版本号（patch）**：每升级 10 次就推动升级一次二级版本号
### 三级版本号（patch）升级触发（2026-11-16 口径更新）

**需求池每 3 个可开发的需求 → 推进一次三级版本号（patch）升级**（release-check.ps1 的「待开发 ≥3 触发发布」即此规则的自动体现）；或**用户在会话中指定专门要求发布版本升级**。patch 每累计 10 次推动一次 minor 升级。

4. **里程碑补充规则**：需求池累计需求记录每满 10 条 → 本次发布提前升一次二级版本号（由 `scripts/release-check.ps1` 依据 `meta.milestoneCount` 自动判定，`scripts/next-version.ps1 -Minor` 计算；发布完成后更新基线计数）

本规则已固化于：`scripts/next-version.ps1` 头注释、`scripts/release-check.ps1` 头注释、`docs/产品开发手册.md`。

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
| 8 官网 | ① **全页版本串自动替换**：除 changelog.html 外全部页面（含资产表格文件名/下载链接/meta 描述）旧版本串统一替换为本次版本（alpha 历史名保护）；② **版本徽章同步**（main.js SITE_VERSION + ?v=N 自动递增）；③ 全页上传；④ downloads/ 五件上传；⑤ **8a 前置校验**（上传前，失败即终止）：changelog.html 必须已人工撰写 v$VER 条目且「当前构建」徽章迁移到新块、download.html Release 链接与描述已指向新版本；⑥ **8b 线上逐条校验**（12 项：徽章常量/changelog 条目/当前构建位置/旧块摘除/Release 链接/下载页描述/首页 main.js 引用/downloads 五件逐一 200），**任一失败 exit 1 = 发布流程未完成**；⑦ HTML 已配置 no-cache 协商缓存（2026-11-16），普通 F5 即见新版 | 官网 sync/deploy 步 + 逐条校验（2026-11-16 强化）|

> **发布者职责（第 8 步前必须完成）**：仅两处人工内容——① `easyrdp-site/changelog.html` 撰写新版本条目（对齐 README 版本小节）并把「当前构建」徽章从旧版本块迁到新块；② README.md 版本小节（release.sh 自动提取为 Release 说明）。其余页面的版本串由脚本自动替换。

## 发布后逐条校验清单（8b 自动执行，全过才算发布完成）

1. 线上 main.js 徽章常量 = 本次版本
2. changelog.html 含新版本条目
3. 「当前构建」徽章位于新版本块
4. 旧版本块已摘除「当前构建」
5. download.html GitHub Release 链接指向新 tag
6. download.html 描述已更新
7. 首页引用最新 main.js?v=N
8. downloads/ 五件逐一 HTTP 200

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



## 灾备包刷新节点（DR-REFRESH，2026-11-18 新增）

**规则：仅二级版本（Y 位）发布时刷新灾备包；patch（Z 位）发布不刷新。**

- 触发条件：版本号 Y 位递增（如 v0.3.3 → v0.4.0）。patch（如 v0.3.3 → v0.3.4）跳过。
- 刷新内容（双角色）：开发机（全量源码含最新站点/后台 + secrets-vault 加密库 + tools 构建依赖 + 一键安装脚本）、远程服务器（webroot 全量含最新 API/页面 + easyrdp-data + keys.php 机密）。
- 产出：`B:\EasyRDP-DR-<日期>\` 目录 + 双角色 zip；远程角色 zip 同时在 sv 服务器保留一份（/root/dr-remote-server-<版本日期>.zip，离线异地副本）。
- 制作方式：sv 端组装远程角色密码 zip（避开开发机杀软对 keys.php 的拦截，参见 scripts/sv-build-dr-zip-v3.sh）；开发机角色按 dev-machine 目录组装 zip（.NET ZipFile 流式，PS5.1 Compress-Archive 处理 3GB 会卡死不可用）。
- 应急：即使未到 minor 节点，若生产配置发生重大变更（如新增机密/新增定时任务/服务器迁移），可手动刷新一次并在本文档记录。

## 官网更新记录节点（第 8 步强制动作，2026-11-18 新增）

每次第 8 步官网同步完成后，必须维护「官网更新记录」体系（后台"🚀 官网更新记录"标签页与官网页脚版本号的数据源）：

1. **事件记录**：每次官网内容/功能/配置更新，在 `docs/官网更新历史记录.md` 追加事件（格式见文档内既有条目：
   `## vX.Y.Z（日期）主题` + `- **事件标题（日期）**：明细`；官网版本号规范 vX.Y.Z：X=全站架构改版/Y=新功能模块/Z=修复微调）。
2. **生成**：`node easyrdp-site/tools/gen-site-updates.js`（解析历史记录 md → `easyrdp-site/assets/data/site-updates.json`）。
3. **上传**：`site-updates.json` 随第 8 步页面上传（scp 到 `$SITE_DIR/assets/data/`，chown www:www）。
4. **触发同步**：`/root/easyrdp-sync-to-sv.sh`（内容随 10 分钟自动同步亦可）。

> 注意：此记录是**官网（网站本身）的更新历史**，与软件版本更新日志（changelog.html/README 版本节）是两回事；
> 官网版本号独立递增，不与软件版本号同步。
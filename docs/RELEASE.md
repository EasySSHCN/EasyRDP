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

## 与 EasySSH 流程的差异（EasyRDP 特化）

1. **构建端**：Windows Rust/Tauri 静态链接（单 exe），非 node/electron 打包
2. **资产**：五件套（源码 tar 也作为 Release 资产发布，alpha 起的惯例）；EasySSH 只发 zip
3. **GitHub git 凭据形态**：`x-access-token:<token>` 用户 + `http.version=HTTP/1.1`
   ——本机实测 `EasySSHCN:<token>` 认证失败、HTTP/2 有 curl 16 framing 层错误
4. **Gitee 同 tag 语义**：删除重建（全量替换资产）；EasySSH 是复用追加
5. **校验步**：发布前强制资产完整性检查（EasySSH 无此步）
6. 暂无官网自动同步步（EasyRDP 官网 easyrdp-site 手动发布；待接入 sync/deploy）

## 凭据保管

- 位置：`secrets-vault/{creds.enc, key}`（隐藏目录 + .gitignore，**禁止外泄/提交**）
- 格式：`creds.enc` = AES-256-CBC(PBKDF2) 密文，内容两行 `github_api_token:` / `gitee_api_token:`；`key` = 64 hex 字符密钥
- 更新令牌：见 `secrets-vault/README.txt`
- 灾备：`make-dr-package.ps1` 会把 secrets-vault 一并打包

## 历史脚本

- `tools-*.sh`（alpha 期 GitHub/Gitee 操作脚本）保留作参考；发布一律走 `scripts/release.sh`

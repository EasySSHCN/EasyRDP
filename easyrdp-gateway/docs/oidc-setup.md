# EasyRDP 网关 OIDC/SSO 真机对接清单（F5-V2, R-20261118-018）

网关内置 Web 控制台（`/`）+ OIDC Authorization Code 流（`/oidc/login` →
IdP → `/oidc/callback`）。本文记录对接真实 IdP（Keycloak / 企业微信 SSO /
Auth0 / 自建 Dex 等）所需的配置与登记步骤。

## 1. config.toml [oidc] 段

```toml
[oidc]
issuer          = "https://sso.example.com/realms/easyrdp"   # 必填，与 id_token.iss 精确一致
discovery_url   = ""                                          # 可选，缺省 = {issuer 去尾斜杠}/.well-known/openid-configuration
client_id       = "easyrdp-gateway"                           # 必填
client_secret   = "<>=32 字节随机值>"                          # 必填（client_secret_post 方式提交）
redirect_uri    = "https://gw.example.com/oidc/callback"      # 必填，须在 IdP 预先登记
scopes          = "openid profile email roles"                # 可选，缺省 "openid profile email"
roles_claim     = "roles"                                     # 可选，缺省 "roles"；缺失回退 "groups"，再回退 ["user"]
allow_placeholder_login = false                               # 可选；配置 [oidc] 后占位登录默认禁用，true 仅限联调
```

- 四个必填字段（issuer/client_id/client_secret/redirect_uri）任一缺失 →
  OIDC 不启用，占位登录保持可用（向后兼容）。
- id_token 签名算法支持 RS256 / ES256；header 必须带 `kid` 且能在 JWKS 中
  匹配，否则拒绝。校验项：签名、`iss`、`aud=client_id`、`exp`（leeway 60s）。
- JWKS / discovery 各缓存 10 分钟 / 1 小时；IdP 轮换密钥后最长 10 分钟生效
  （如需立即生效重启网关）。

## 2. Keycloak 步骤（示例）

1. Clients → Create：Client ID = `easyrdp-gateway`，Client authentication
   ON（confidential），Standard Flow 勾选。
2. Valid redirect URIs 登记：`https://gw.example.com/oidc/callback`
   （必须与 [oidc].redirect_uri 逐字符一致，含 scheme/路径/大小写）。
3. Web Origins 填网关域名（或 `+`）。
4. Credentials → Client secret 复制到 `client_secret`。
5. 角色下发：把客户端角色（或 realm roles）映射进 token——推荐在客户端
   scope 加 **roles** scope（默认已有）；如角色在其他 claim（如 `groups`），
   调整 `roles_claim`。
6. issuer 核对：Realm Settings → Endpoints → OpenID Endpoint Configuration
   的 `issuer` 字段必须与配置一致（尾斜杠差异会导致校验失败）。

## 3. 企业微信 SSO（WeCom OAuth2）注意点

企业微信不是标准 OIDC 提供方时，需要前置一个兼容层（如 Dex、自建
translation gateway）把 oauth2 code 换取流程转换为标准 OIDC：
- discovery / JWKS / id_token 三件套齐备；
- 用户身份映射到 `sub`，部门/角色映射进 `roles` 或 `groups` claim；
- redirect_uri 在企业微信管理后台「可信域名 + 授权回调域」登记。

## 4. 证书信任

- 网关 → IdP（discovery/token/JWKS 拉取）：reqwest + rustls + **webpki-roots**
  （内置公共 CA）。IdP 用企业内网 CA 签发时需把根证书加入系统信任库
  （Windows：certlm.msc → 受信任的根证书颁发机构），reqwest 走
  rustls-native-certs?——当前实现用 webpki-roots，**内网 CA 场景请将 IdP
  换公共 CA 证书或后续扩展 custom root**（见遗留项）。
- 浏览器 → 网关：`[tls]` 配置网关证书（或 Nginx 反代终止 TLS）。
- redirect_uri 必须是 https（生产）；本地联调可用 http://localhost:8443。

## 5. 上线核对清单

- [ ] redirect_uri 已在 IdP 登记，且与 config.toml 一致
- [ ] issuer 与 discovery 文档 issuer 精确一致（含尾斜杠）
- [ ] client_secret 为 >=32 字节随机值，不入库不入 git
- [ ] allow_placeholder_login 未开启（默认禁用占位登录）
- [ ] 浏览器实测：/ → OIDC 登录 → IdP 登录 → 回跳控制台 → 登出
- [ ] 本地 totp_secret 用户实测：OIDC 登录后出现本地 OTP 二次认证页
- [ ] 审计核对：console_login / console_logout / oidc_callback_failed 事件

## 6. 回滚

把 [oidc] 段注释/删除 → 重启网关即回退占位登录形态（历史会话 cookie
仍有效直至过期）。

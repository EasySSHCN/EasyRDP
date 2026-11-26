# R-20261118-009「JSON Profile Schema 校验」交付报告

日期：2026-11-18 ｜ 状态：完成（仅开发+自测，未 git/未发布/未部署）

## 一、改动文件清单

| 文件 | 改动 |
|---|---|
| `crates/easyrdp-core/src/schema.rs` | **新增**：draft-07 Schema 常量 + 手写轻量校验器（19 个单测） |
| `crates/easyrdp-core/src/lib.rs` | 注册 `pub mod schema`，导出 `profile_json_schema / validate_profile_json / JsonProfileIssue / Severity / ValidationMode` |
| `crates/easyrdp-core/examples/linkdesk.rs` | **新增**：`linkdesk schema [OUT_FILE]` / `linkdesk validate <file> [--strict]`（仓库无 CLI 二进制，按约定落 example） |
| `src-tauri/src/main.rs` | `ConnectionStore::import_from_file` 接入导入前 Schema 校验（宽松模式）；新增导入坏条目单测；导入测试夹具补 host；顺手修复并行改动遗留的 `merge_batch` move-after-borrow 编译错误（`failed: candidates.len()` → 循环前取 `total`） |

## 二、Schema 摘要

- draft-07，`$id: https://easyrdp.example/schemas/profile-v1.json`，根 `type: object`，`additionalProperties: false`，`required: ["host"]`。
- 顶层 10 个 properties 与 `SessionProfile` serde 字段一一对应：`name, host, port(1-65535, default 3389), username, auth_method(password|sso|smart_card), resolution(fit_window|fullscreen|{"custom":{w,h}}), color_depth(16|24|32), order, redirect{clipboard,printers,drives,camera}, gateway(null | gatewayConfig)`。
- `definitions.gatewayConfig`：hostname/port(1-65535,default 443)/use_tls/auth{type: otp_cookie|inherit|token|user_password}/resource_id/token/username/password（后三者为 string|null）。
- 固化防漂移单测：schema properties ↔ 模型 serde 序列化键集合逐一断言（`schema_properties_match_model_fields`、`schema_subobject_fields_match_model`、`schema_defaults_match_model_default`、`schema_port_bounds_match`）。模型加字段（如本次并行的 `order: i64`）会导致该测试失败并提示同步。
- 生成方式：`cargo run -p easyrdp-core --example linkdesk -- schema out.json`。

## 三、校验器设计

- **手写轻量实现，未引外部 jsonschema 库**（零新增依赖；仅校验本模型一个固定 schema，引库收益低且拉大依赖树）。
- 在 serde 反序列化**之前**对原始 `serde_json::Value` 校验（serde default 会静默吞错型字段，前置才能发现 `port:"3389"` 这类问题）。
- 问题分类：`Severity::Error`（缺 host / 类型错 / 端口越界 / 枚举非法 / 分辨率非法）与 `Severity::Warning`（未知字段）。
- 每条问题带 JSON Pointer（RFC 6901）：`#/host`、`#/port`、`#/redirect/clipboard`、`#/gateway/auth/type` 等，`Display` 输出 `指针: 消息`。
- 模式：`Lenient`（默认，硬错误失败、未知字段警告放行）/ `Strict`（任何问题全量失败）。
- 导入链路：Schema 校验（逐条，失败条目跳过并在返回错误中逐条列出，**不全量失败**）→ serde 反序列化 → 既有 `SessionProfile::validate()` → 入库（凭据加密逻辑不变）。

## 四、测试结果（全部绿）

```
cargo test -p easyrdp-core -p easyrdp-app   → EXIT=0
easyrdp-core: 76 passed / 0 failed（其中 schema::tests 19 个）
easyrdp-app : 54 passed / 0 failed / 1 ignored
```

覆盖：schema 固化×5、正/反向校验（缺 host、空 host、port 字符串、port 越界、未知字段警告、嵌套 gateway 字段、redirect 类型、auth_method 非法枚举、非对象根）、strict/宽松×2、模型↔校验器回环、app 导入坏条目拒绝+好条目入库。

## 五、官网文档口径核对（只读，未改 easyrdp-site）

`docs.html` §JSON Profile 描述的是一套**规划版**字段，与实现（`SessionProfile`）严重不一致：

| # | 文档口径 | 实现口径 | 结论 |
|---|---|---|---|
| 1 | `auth: {type, username, store}` 嵌套对象；type 含 `"nla"` | 扁平 `username` + `auth_method: password\|sso\|smart_card` | 不一致（文档 `"nla"` 在校验器中为非法枚举） |
| 2 | `redirection: {clipboard, drive, printer, usb, audio}` | `redirect: {clipboard, printers, drives, camera}`（无 usb/audio，字段名复数） | 不一致 |
| 3 | `session: {resolution:"1920x1080", multiMonitor, colorDepth}` | `resolution: fit_window\|fullscreen\|{"custom":{w,h}}` + 顶层 `color_depth` | 不一致（字符串分辨率格式不存在；文档 rdp-import 映射表同样引用旧口径） |
| 4 | `gateway: null`（标注"V1 路线图字段，预留"） | `gateway` 已完整实现（GatewayConfig，含 port/use_tls/auth/resource_id/token） | 文档落后于实现 |
| 5 | 未提 `port` 默认值外的 `order` 字段 | 模型含 `order: i64`（并行改动新增） | 文档缺字段 |
| 6 | CLI 节列出 `connect/import/list/export` | 仓库无 `linkdesk` CLI 二进制（本次补了 `schema`/`validate` example） | 文档超前于实现 |

## 六、遗留项

1. 文档五项不一致需站点侧更新（本次按约束未改 easyrdp-site）。
2. `import_from_file` 目前固定 Lenient；如需 strict 全量失败语义，可在前端/`import_connections` command 暴露 `--strict` 参数（核心 API 已就绪）。
3. example 输出的 schema 未持久化到仓库固定路径（本次写入 `target/test-data/profile.schema.json` 冒烟验证）；如需随包分发 schema 文件可后续落 `docs/profile.schema.json`。
4. 并行改动说明：开发期间 `SessionProfile` 被另一条线新增了 `order: i64` 字段，schema 与校验器已同步覆盖，防漂移单测会持续守住后续同步。

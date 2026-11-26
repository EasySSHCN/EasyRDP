# F2-V2 团队模板库（R-20261118-014）交付报告

## 实现口径
模板 = `TemplateProfile`（SessionProfile 可共享子集 + 元数据），凭据绝不入模板；
导出门禁拒绝任何凭据/密文；`.erdp-template`（单）/ `.erdp-templates`（多）包带
schema 版本 + sha256 完整性；导入走「校验 → 预览 → 入库或预填新建连接」双路径。

## 改动清单
- `crates/easyrdp-core/src/template.rs`（新增）：TemplateMeta / TemplateProfile /
  from_session（剥离网关 token/password/username/otp，主 username 保留可选）/
  find_credential_issues（非空 password、网关 token/password/otp_code + 全树
  `dpapi:`/`plain:` 前缀扫描）/ export_single / export_multi / parse_package
  （sha256 → schema → 逐条校验，复用 schema.rs 宽松校验器）/ partition_duplicates
  （按 meta.id 去重）/ new_template_id / now_utc_string。
- `crates/easyrdp-core/src/lib.rs`：注册 template 模块 + re-export。
- `crates/easyrdp-core/Cargo.toml`：新增 `sha2 = "0.10"`（Cargo.lock 已有该版本，
  零新增下载）。
- `src-tauri/src/main.rs`：TemplateStore（`%APPDATA%\EasyRDP\templates.json`）、
  TemplateView（含 usage 统计）、命令 `list_templates` /
  `save_template_from_connection` / `delete_template` / `template_export` /
  `template_import_preview` / `template_import_commit`；3 条单测。
- `src/index.html`：侧栏「模板库」按钮、模板库弹窗（列表 + 导入按钮 + 空态）、
  模板包导入向导弹窗；style/main.js 缓存版本号递增。
- `src/ui/main.js`：模板库列表渲染、基于模板预填表单（凭据留空）、导出、删除、
  存模板（连接项新按钮）、导入向导（勾选/重复高亮/结果摘要）。
- `src/ui/style.css`：.tpl-actions/.tpl-hint/.tpl-desc/.tpl-ops/.tpl-empty
  （复用深色主题 var token 与 imp-table 族样式）。
- i18n 四字典（zh-CN / zh-TW / en / ko）：`tpl.*`、`side.templates`、
  `conn.saveTpl` 等 27 个新 key。

## 文件格式说明
单模板 `.erdp-template`：
```json
{ "schema": "easyrdp-template-v1", "template": { "meta": {...}, "profile": {...} }, "sha256": "<64hex>" }
```
多模板 `.erdp-templates`：同结构，`templates` 为数组。
`sha256` = 去掉 sha256 字段后的包体（serde_json BTreeMap 键序规范化字符串）的
SHA-256。meta 字段：id（tpl-<纳秒>，去重键）/ name / description / tags /
created_at（UTC） / version（从 1 起）。profile 内 gateway 仅保留
hostname/port/use_tls/auth/resource_id/allow_self_signed，凭据字段一律 null。

## 测试结果
- `cargo test -p easyrdp-core --lib`：111 passed / 0 failed（基线 77 不回归；
  本特性新增 15 条：凭据剥离、导出拒绝 password/token/otp/dpapi:/plain:、
  篡改包 sha256 失败、schema 版本拒绝、单/多包无损往返、导入按 id 去重、
  serde 旧包兼容、Value 再序列化兼容）。
- `cargo test -p easyrdp-app --bin easyrdp-app`：70 passed / 0 failed / 1 ignored
  （基线 56 不回归；本特性新增 3 条：templates.json 持久化回读、
  导出→导入回环 + 同 id 去重 + 视图无凭据 + usage 统计、凭据模板导出被拒）。
- `node --check`：main.js 与四个语言包全部通过。

## 遗留项
1. 「直接创建连接」路径：MVP 采用「基于模板预填表单 → 用户走既有保存/连接
   流程」实现（表单预填即 F2-V1 语义的凭据留空），未复用批量向导的多选分组
   提交链路；后续可加「一键批量从模板建连接」。
2. 模板编辑/重命名、标签筛选、模板包内逐条签名（目前仅包级 sha256）未做。
3. 并行开发说明：同仓并行会话在进行 F6-V2 git_sync 与 snapshot 工作，其测试
   已在本轮验证时全绿；本特性与其无接口冲突（templates.json 已纳入其同步文件
   清单）。

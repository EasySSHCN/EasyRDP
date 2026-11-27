EasyRDP 集中回归验收 — 准备与基线代理结构化汇报
日期: 2026-11-19  代理: regress-prep

## 1. 版本号
- easyrdp-client\src-tauri\tauri.conf.json version = 0.3.13（符合预期 0.3.13）
- workspace Cargo.toml (easyrdp-client\Cargo.toml) workspace.package.version = 0.3.13
- easyrdp-gateway\Cargo.toml version = 0.3.13

## 2. 备份
- 备份路径: %APPDATA%\EasyRDP\regression-bak-20261119\
- 说明: 源目录中不存在 connections.json、groups、sync、templates 等数据文件
  （该目录当前仅含 debug-*.log、freerdp.log、freerdp-config、present-audit-run1、
  snapshots、updates）。已全量复制现存全部内容（29 个文件/目录），未遗漏。

## 3. cargo test（easyrdp-client workspace 全量）
命令: 在 easyrdp-client 目录执行 `cargo test`
结果: 全部通过，0 失败。
- easyrdp-app (src-tauri bin): 71 passed, 0 failed, 1 ignored (updater::ota_probe_tests::verify_live_manifest_rust_semantics)
- easyrdp-core: 115 passed, 0 failed  ← 基线 ≥111 passed 满足 ✓
- easyrdp-pixels: lib 单测 0；tests/correctness.rs 7 passed, 0 failed
- easyrdp-rdp: 28 passed, 0 failed
- doc-tests: 全部 0
- 合计: 221 passed / 0 failed / 1 ignored
- 编译警告若干（unused import/dead_code 等，均非失败），构建耗时 40.55s。
- 日志: EasyRDP\regress-client-test.log

## 4. easyrdp-pixels / easyrdp-gateway 覆盖情况
- easyrdp-pixels: 在 easyrdp-client workspace 内（Cargo.toml members 含 crates/easyrdp-pixels），
  已被 cargo test 一并覆盖（correctness 7 通过）。无独立 crate 测试需另跑。
- easyrdp-gateway: 不在 easyrdp-client workspace 内，独立 crate，已单独执行 `cargo test`。

## 5. 失败清单
### F1: easyrdp-gateway 构建失败（ring 构建脚本）
- 复现命令: cd easyrdp-gateway; cargo test
- 错误: failed to run custom build command for `ring v0.17.14`（exit code 1）
- 关键输出（3-5 行）:
  cl: 命令行 error D8050 : 无法执行
  "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\c1.dll":
  未能将命令行放入调试记录中
  error occurred in cc-rs: command did not execute successfully ... cl.exe ... -c ring-0.17.14\crypto/curve25519/curve25519.c
  warning: build failed, waiting for other jobs to finish...
- 备注: 疑似 PATH 过长（环境变量超长导致 cl D8050），仅记录，未修复。
  日志: EasyRDP\regress-gateway-test.log
- 因此 gateway 测试用例数为 0（未能编译，无法运行）。

## 6. 结论
- 客户端基线达标: easyrdp-core 115 passed ≥ 111 ✓；workspace 全量 0 失败。
- 唯一待办遗留: easyrdp-gateway 因 ring/MSVC D8050 构建失败，测试未运行（仅记录）。

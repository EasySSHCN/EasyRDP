# ⚠️ v0.3.11 开发基线交接（2026-11-18 · lead 会话 → v0.3.11 开发会话）

## 结论先行

**v0.3.10 发布版缺两处 UI 补丁**（另一会话打包时基线里没有）。我已把两处补丁**精确移植到 v0.3.10 源码基线**，放在本目录：

```
EasyRDP/handoff-0.3.11-baseline/easyrdp-client-src/   ← 带补丁的 client src（以官方 v0.3.10 为基线）
```

**v0.3.11 请以这份 src 为起点**（或把下面的 diff 合进你手里的基线），否则两处改动会在下一版再次漏发。

## 补丁 1：升级横幅内全流程（对齐 EasySSH updater.js 模式）

文件：`ui/settings.js`
- 新增 `bannerProgressBound` / `bindBannerProgress()`：监听 `update-progress`，把阶段/百分比写进横幅文本与内嵌进度条
- `ensureUpdateBanner()` 重写：内嵌 `.ub-prog/.ub-bar` 进度条元素；「立即升级」直接 `invoke("download_and_install")`，进度/结果/失败重试全部在横幅内完成，**不再跳设置页**（旧代码点了不存在的 `set-update-btn`，且面板选择器失配会跳到常规标签页）
- CSS 增加 `.ub-btn:disabled` 规则（在 JS 内注入的 style 里）

## 补丁 2：网关（可选）友好提示 + 完整地址粘贴解析

文件：`index.html` / `ui/main.js` / `ui/i18n-dict-{zh-CN,zh-TW,en,ko}.js` / `ui/style.css`
- `index.html` 网关折叠区：summary 加灰字说明（`form.gw.sumNote`）；展开首屏加完整提示段（`form.gw.tip`：官方网关 easyrdp.cn / 443 / TLS、认证方式区别、资源 ID 含义）；全部 placeholder 与选项接 `data-i18n-ph` / `data-i18n`
- `ui/main.js`：`f-gw-host` input 监听——粘贴 `https://gw:8443` / `wss://…` 自动拆域名/端口/TLS；纯主机名不受影响
- 四语言字典 +12 条：`form.gw.sumNote/tip/tlsOn/tlsOff/authToken/authUserPass`、`form.ph.gwHost/gwToken/gwUser/gwPass/gwResource`
- `ui/style.css`：`.gw-sum-note` 与 `#gateway-section .note` 两条规则（紧跟 `.note` 定义之后）

## 移植方式（可复核）

对 v0.3.10 官方源码（easyrdp.cn/downloads/EasyRDP-src-0.3.10.tar.gz）做**锚点编辑**，非整文件覆盖——v0.3.10 的 R-20261118-007 收藏栏、style 版本号 2026111801 等新特性全部保留未动。全 JS `node --check` 通过；`bindBannerProgress` / `gw-sum-note` / `form.ph.gwHost` / 粘贴解析四项特征 PRESENT。

## 教训（避免重演）

跨会话并行开发时，UI 补丁必须落到**双方可见的共享基线**（本工作区/仓库），只改自己会话的 TEMP 解包树必漏合。v0.3.11 发版前建议 grep 校验：`bindBannerProgress`、`form.gw.tip` 两键必须存在。

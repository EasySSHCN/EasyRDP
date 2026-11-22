$subject = "EasyRDP v0.4.2 发布通知（更正重发）"
$text = @"
EasyRDP v0.4.2 已发布（2026-11-21）。

本版更新（7 条随版发布）：
- 在线升级「文件被占用」自动接力闭环：替换被占用时自动重启完成落位、新版自动拉起、快捷方式自动改指，全程零人工二次点击（R-20261120-003）
- 遥测上报频控改造：软件更新新版后每次打开必上报一次动态；同版本保留按日去重（R-20261120-004）
- 修复「导出」按钮悬停手册链接错指：改指 export-json.html（R-20261120-006）
- 官网全站页脚展示版本号并随发布自动同步（R-20261120-007）
- 网关安全修复（P0）：占位登录公网暴露封堵，allow_placeholder_login 生产关闭，公网实测拒绝（R-20261120-008）
- 用户侧云网关服务台上线：https://easyrdp.cn/gw-admin.html 注册/登录后自助签发短期 Token、查看资源、吊销授权（R-20261120-009）
- 手册与 FAQ 云网关口径修正（R-20261120-010）

发布核验：
- 官网 8b 线上校验 15/15 全过，downloads 五件 200
- GitHub：https://github.com/EasySSHCN/EasyRDP/releases/tag/v0.4.2
- Gitee：https://gitee.com/easyssh/EasyRDP/releases/tag/v0.4.2
- OTA latest.json 已上线（latestVersion=0.4.2）
- 网关 v0.4.2（Linux）已部署生产并重启生效，占位登录公网 403
- gw-admin.html 注册/登录链路冒烟验证通过

待办：R-20261120-005（来源归因对齐 EasySSH）未开发、需 EasySSH 基线盘点，保留至下批发布。

下载：https://easyrdp.cn/download.html
"@
powershell -ExecutionPolicy Bypass -File "B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\scripts\notify-email.ps1" -Subject $subject -Text $text

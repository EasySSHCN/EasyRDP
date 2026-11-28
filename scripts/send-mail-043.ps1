$subject = "EasyRDP v0.4.3 发布通知"
$text = @"
EasyRDP v0.4.3 已发布（2026-11-21）。

本版更新：
- 来源归因对齐 EasySSH（R-20261120-005）：服务端四级归因管线（票据精确 high/超窗 low → 同 IP 票据扫描 → 方案 B 启发式 → IP 段兜底），origin 全字段（来源分类/来源链接/下载时间/归因时间/访问流水联查/置信度）
- 下载票据签发携带来源分类（github/gitee/search/direct/other）
- 后台客户端统计新增「来源分布」与设备明细「来源归因」区块，归因方式三档文案（票据归因/启发式/IP 段匹配，疑似标注）
- 本版客户端无功能变化，随版构建收口

发布核验：
- 官网 8b 线上校验 15/15 全过，downloads 五件 200
- GitHub：https://github.com/EasySSHCN/EasyRDP/releases/tag/v0.4.3
- Gitee：https://gitee.com/easyssh/EasyRDP/releases/tag/v0.4.3
- OTA latest.json 已上线（latestVersion=0.4.3）
- 线上 E2E 冒烟通过（百度来源票据 → search 分类 → high 票据归因 → attributed 标记）

需求池当前无待发布条目（release-request.json 已归档删除）。

下载：https://easyrdp.cn/download.html
"@
powershell -ExecutionPolicy Bypass -File "B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\scripts\notify-email.ps1" -Subject $subject -Text $text

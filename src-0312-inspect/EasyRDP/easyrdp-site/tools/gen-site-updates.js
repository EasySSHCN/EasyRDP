#!/usr/bin/env node
// gen-site-updates.js —— 从 docs/官网更新历史记录.md 解析生成 assets/data/site-updates.json
// （后台「官网更新记录」标签页 + 官网页脚版本号的数据源；CHANGELOG-SITE 2026-11-18）
// 用法：node tools/gen-site-updates.js [md路径] [输出路径]
//   默认：../docs/官网更新历史记录.md → ../assets/data/site-updates.json（相对本工具目录）
const fs = require("fs");
const path = require("path");

const toolDir = __dirname;
const mdPath = process.argv[2] || path.resolve(toolDir, "../../docs/官网更新历史记录.md");
const outPath = process.argv[3] || path.resolve(toolDir, "../assets/data/site-updates.json");

const raw = fs.readFileSync(mdPath, "utf8");
const lines = raw.split(/\r?\n/);

// 版本号规范行（## 官网版本号规范 小节首段）
let scheme = "";
// 事件明细行：- **标题**：明细
const versions = []; // {ver, date, summary, items:[{title, detail}]}
let cur = null;

for (const line of lines) {
  const verM = line.match(/^##\s+(v\d+\.\d+\.\d+)（([^）]+)）\s*(.*)$/);
  if (verM) {
    cur = { ver: verM[1].replace(/^v/, ""), date: verM[2].trim(), summary: (verM[3] || "").trim(), items: [] };
    versions.push(cur);
    continue;
  }
  const schemeM = line.match(/语义化版本 vX\.Y\.Z：(.+)$/);
  if (schemeM && !scheme) { scheme = "语义化版本 vX.Y.Z：" + schemeM[1].trim(); continue; }
  if (!cur) continue;
  const itemM = line.match(/^- \*\*(.+?)\*\*[:：]\s*(.*)$/);
  if (itemM) cur.items.push({ title: itemM[1].trim(), detail: itemM[2].trim() });
}

const total = versions.reduce((n, v) => n + v.items.length, 0);
const updatedAt = versions[0] ? versions[0].date : new Date().toISOString().slice(0, 10);
const out = {
  current: versions[0] ? versions[0].ver : "1.0.0",
  updatedAt,
  scheme: scheme || "语义化版本 vX.Y.Z：X=全站架构/改版级变更；Y=新功能模块上线；Z=缺陷修复与样式文案微调。",
  total,
  versions,
};

fs.mkdirSync(path.dirname(outPath), { recursive: true });
fs.writeFileSync(outPath, JSON.stringify(out, null, 2) + "\n", "utf8");
console.log(`site-updates.json 生成完成：current=${out.current} versions=${versions.length} total=${total} → ${outPath}`);

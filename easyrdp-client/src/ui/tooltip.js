// 功能按钮悬停说明 tooltip + 官网手册外链（R-20261119-003，v0.4.0）。
//
// 行为：
//   - 鼠标悬停/键盘聚焦目标功能按钮时弹出极简说明层（说明文本复用原
//     data-i18n-title 词条，四语言随当前语言即时取词）；
//   - 说明尾部固定「详细手册 ↗」链接，点击调用 Rust open_manual_url
//     （ShellExecuteW 系统默认浏览器打开，禁止内嵌 webview；Rust 侧仅放行
//     easyrdp.cn 域）；
//   - 按钮 → 手册页 URL 在 MANUAL_LINKS 集中维护，官网各功能手册页
//     一一对应（easyrdp-site/manual/<page>.html），后续同步只改这一处。
//
// 原生 title 冲突处理：初始化时把目标按钮的 data-i18n-title 重命名为
// data-tip 并清掉 el.title（applyI18n 只处理 data-i18n-title，不再回写），
// 避免「原生 title + 自定义 tooltip」双层弹出。

import { t } from "./i18n.js?v=2026111602";

// ---- 功能 → 官网手册页映射（集中维护，与 easyrdp-site/manual/ 一一对应）----
const MANUAL_BASE = "https://easyrdp.cn/manual/";
const MANUAL_LINKS = {
  "btn-settings": "settings.html",       // 设置面板
  "btn-feedback": "feedback.html",       // 使用反馈
  "btn-import": "import-rdp.html",       // 导入 .rdp
  "btn-new-conn": "connections.html",    // 新建连接
  "btn-new-group": "groups.html",        // 分组管理
  "btn-export": "export.html",           // 导出连接
  "btn-import-json": "import-json.html", // 导入 JSON
  "btn-templates": "templates.html",     // 模板库
  "rdp-fullscreen": "fullscreen.html",   // 会话全屏
  "rdp-win": "win-key.html",             // 发送 Win 键
  "rdp-disconnect": "disconnect.html",   // 断开会话
};

let tipEl = null;
let hideTimer = null;

function ensureTipEl() {
  if (tipEl) return tipEl;
  tipEl = document.createElement("div");
  tipEl.id = "manual-tip";
  tipEl.className = "manual-tip hidden";
  tipEl.setAttribute("role", "tooltip");
  document.body.appendChild(tipEl);
  return tipEl;
}

function showTip(btn) {
  const key = btn.dataset.tip;
  if (!key) return;
  const page = MANUAL_LINKS[btn.id];
  const el = ensureTipEl();
  el.innerHTML = "";
  const text = document.createElement("span");
  text.className = "manual-tip-text";
  text.textContent = t(key);
  el.appendChild(text);
  if (page) {
    const link = document.createElement("a");
    link.className = "manual-tip-link";
    link.href = "#";
    link.textContent = ` ${t("tip.manual")}`;
    link.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      hideTip();
      try {
        window.__TAURI__.core.invoke("open_manual_url", { url: MANUAL_BASE + page })
          .catch((e) => console.warn("打开手册失败:", e));
      } catch { /* __TAURI__ 不可用（浏览器直开 dev 页）：忽略 */ }
    });
    el.appendChild(link);
  }
  el.classList.remove("hidden");
  // 定位：优先按钮下方左对齐；贴底时改上方；横向贴右时右对齐，且不出视口
  const r = btn.getBoundingClientRect();
  const tw = el.offsetWidth, th = el.offsetHeight;
  let x = r.left;
  let y = r.bottom + 8;
  if (y + th > window.innerHeight - 8) y = r.top - th - 8;
  if (x + tw > window.innerWidth - 8) x = window.innerWidth - tw - 8;
  if (x < 8) x = 8;
  el.style.left = `${Math.round(x)}px`;
  el.style.top = `${Math.round(Math.max(8, y))}px`;
}

function hideTip() {
  clearTimeout(hideTimer);
  if (tipEl) tipEl.classList.add("hidden");
}

/** 初始化：为全部已登记功能按钮挂接 tooltip（幂等，可重复调用）。 */
export function initManualTooltips() {
  for (const id of Object.keys(MANUAL_LINKS)) {
    const btn = document.getElementById(id);
    if (!btn || btn.dataset.tipBound === "1") continue;
    // 原生 title 迁移：data-i18n-title → data-tip（applyI18n 不再回写 title）
    if (btn.dataset.i18nTitle) {
      btn.dataset.tip = btn.dataset.i18nTitle;
      btn.removeAttribute("data-i18n-title");
    }
    btn.removeAttribute("title");
    btn.dataset.tipBound = "1";
    btn.addEventListener("mouseenter", () => {
      clearTimeout(hideTimer);
      showTip(btn);
    });
    btn.addEventListener("mouseleave", () => {
      clearTimeout(hideTimer);
      hideTimer = setTimeout(hideTip, 120);
    });
    btn.addEventListener("focus", () => showTip(btn));
    btn.addEventListener("blur", hideTip);
    btn.addEventListener("mousedown", hideTip); // 拖拽/点击时不遮挡
  }
}

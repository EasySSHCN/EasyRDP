// EasyRDP 连接中心前端。

import { openSession, closeSession, bindInput } from "./rdp-view.js";
import { bootstrapSettings, initSettings } from "./settings.js?v=2026111701";
import { t, applyI18n } from "./i18n.js?v=2026111602";

const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;
const win = window.__TAURI__.window.getCurrentWindow();

// ---- 版本号动态填充（v0.1.2 修复：标题栏 tag 不再硬编码，随 tauri.conf 版本走）----
if (window.__TAURI__?.app?.getVersion) {
  window.__TAURI__.app.getVersion().then((v) => {
    const el = document.getElementById("app-version-tag");
    if (el) el.textContent = `v${v}`;
  }).catch(() => {});
}

// ---- 启动淡入 + 显示窗口（visible:false -> show）----
// CSS transition 0.3s；等首帧淡入开始后再 show，避免 webview 初始化白屏闪现。
requestAnimationFrame(() => {
  document.body.classList.add("ready");
  setTimeout(() => { win.show().catch(() => {}); }, 50);
});

// ---- 自绘标题栏窗口控制（decorations:false）----
document.getElementById("btn-win-min").addEventListener("click", () => win.minimize().catch(() => {}));
document.getElementById("btn-win-max").addEventListener("click", () => win.toggleMaximize().catch(() => {}));
document.getElementById("btn-win-close").addEventListener("click", () => win.close().catch(() => {}));

const $ = (id) => document.getElementById(id);
const listEl = $("conn-list");
const emptyEl = $("conn-empty");
const countEl = $("conn-count");
let connections = [];
let editingId = null;
let selectedId = null;

// ---- 连接状态缓存（localStorage：上次连通性测试结果）----
const STATUS_KEY = "easyrdp-conn-status";
function loadStatus() {
  try { return JSON.parse(localStorage.getItem(STATUS_KEY)) || {}; }
  catch { return {}; }
}
function setStatus(id, state) { // state: "ok" | "fail"
  const s = loadStatus();
  s[id] = { state, at: Date.now() };
  localStorage.setItem(STATUS_KEY, JSON.stringify(s));
}
function clearStatus(id) {
  const s = loadStatus();
  delete s[id];
  localStorage.setItem(STATUS_KEY, JSON.stringify(s));
}

// ---- 表单校验错误（逐条显示）----
function showErrors(errs) {
  const el = $("form-errors");
  el.innerHTML = "";
  if (!errs || !errs.length) { el.classList.add("hidden"); return; }
  for (const e of errs) {
    const li = document.createElement("li");
    li.textContent = e;
    el.appendChild(li);
  }
  el.classList.remove("hidden");
}

function validateForm() {
  const errs = [];
  if (!$("f-host").value.trim()) errs.push(t("err.hostEmpty"));
  const port = Number($("f-port").value.trim());
  if (!Number.isInteger(port) || port < 1 || port > 65535)
    errs.push(t("err.badPort", { v: $("f-port").value.trim() || t("err.portEmpty") }));
  return errs;
}

// R-20261117-003：原 #msg 按钮右侧提示已取消，flash 统一走底部 conn-hint（见下方 flash 定义）

async function refresh() {
  connections = await invoke("list_connections");
  renderList();
}

function renderList() {
  listEl.innerHTML = "";
  // 搜索过滤（前端过滤：按名称/主机/用户名，大小写不敏感）
  const q = ($("conn-search").value || "").trim().toLowerCase();
  const shown = connections.filter((c) =>
    !q ||
    (c.name || "").toLowerCase().includes(q) ||
    (c.host || "").toLowerCase().includes(q) ||
    (c.username || "").toLowerCase().includes(q));
  countEl.textContent = shown.length;
  emptyEl.classList.toggle("hidden", shown.length > 0);
  if (!shown.length) return;
  const statusMap = loadStatus();
  // 按分组分节显示（EasySSH 风格）：未分组排最后
  const groups = new Map();
  const ungrouped = t("group.ungrouped");
  for (const c of shown) {
    const g = (c.group || "").trim() || ungrouped;
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(c);
  }
  const ordered = [...groups.entries()].sort(([a], [b]) => {
    if (a === ungrouped) return 1;
    if (b === ungrouped) return -1;
    return a.localeCompare(b, "zh-CN");
  });
  for (const [group, items] of ordered) {
    const head = document.createElement("div");
    head.className = "group-head";
    head.textContent = `▾ ${group}（${items.length}）`;
    listEl.appendChild(head);
    for (const c of items) renderConnItem(c, statusMap);
  }
}

function renderConnItem(c, statusMap) {
    const st = statusMap[c.id]?.state || "unknown";
    const item = document.createElement("div");
    item.className = "conn-item" + (c.id === selectedId ? " selected" : "");
    item.title = t("conn.title.dblClick");
    const readyBadge = c.rdp_ready
      ? `<span class="badge ready" title="${esc(t("conn.badge.readyTip"))}">${esc(t("conn.badge.ready"))}</span>`
      : `<span class="badge pending" title="${esc(t("conn.badge.pendingTip"))}">${esc(t("conn.badge.pending"))}</span>`;
    const dotTitle = st === "ok" ? t("conn.dot.ok") : st === "fail" ? t("conn.dot.fail") : t("conn.dot.unknown");
    item.innerHTML = `
      <div class="conn-name"><span class="status-dot ${st}" title="${esc(dotTitle)}"></span>${esc(c.name)}${readyBadge}</div>
      <div class="conn-sub">${esc(c.host)}:${c.port}${c.username ? " · " + esc(c.username) : ""}</div>
      <div class="conn-ops">
        <button class="btn sm" data-act="edit">${esc(t("conn.edit"))}</button>
        <button class="btn sm" data-act="exp" title="${esc(t("conn.title.export"))}">${esc(t("conn.export"))}</button>
        <button class="btn sm danger" data-act="del">${esc(t("conn.delete"))}</button>
      </div>`;
    item.addEventListener("click", (ev) => {
      const act = ev.target?.dataset?.act;
      if (act === "edit") { loadToForm(c); ev.stopPropagation(); return; }
      if (act === "exp") { exportRdp(c); ev.stopPropagation(); return; }
      if (act === "del") {
        if (confirm(t("conn.confirmDelete", { n: c.name }))) {
          if (editingId === c.id) resetForm();  // 正在编辑该连接 => 重置表单
          if (selectedId === c.id) selectedId = null; // 已删除的选中项同步清空
          clearStatus(c.id);
          invoke("delete_connection", { id: c.id }).then(refresh).catch((e) => flash(String(e), false));
        }
        // 用户取消确认框：不触碰 selectedId / editingId，界面保持原状
        ev.stopPropagation(); return;
      }
      selectedId = c.id;
      loadToForm(c);          // 单击即回填表单，便于查看/编辑
      renderList();
    });
    item.addEventListener("dblclick", async () => {
      loadToForm(c);          // 双击：自动填入表单 + 直接进入远程画面
      await startSession(c);
    });
    listEl.appendChild(item);
}

// 导出单条连接为 .rdp 文件（mstsc 兼容；后端不写密码字段）
async function exportRdp(c) {
  try {
    const path = await dialog.save({
      defaultPath: `${c.name || c.host}.rdp`,
      filters: [{ name: "RDP 配置", extensions: ["rdp"] }],
    });
    if (!path) return;
    await invoke("export_connection", { id: c.id, path });
    flash(t("msg.exportedOne", { n: c.name || c.host, p: path }));
  } catch (e) { flash(String(e), false); }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (m) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[m]));
}

function loadToForm(c) {
  editingId = c.id;
  showErrors(null);
  $("form-title").textContent = t("form.editPrefix", { n: c.name || c.host });
  $("f-id").value = c.id;
  $("f-name").value = c.name;
  $("f-host").value = c.host;
  $("f-port").value = c.port;
  $("f-username").value = c.username;
  $("f-password").value = "";
  $("f-group").value = c.group || "";
  $("f-resolution").value = c.resolution_kind;
  // 2026-11 UI 修复：程序化设置 select.value 不触发 change 事件——手动
  // toggle custom-res 显示并回填已保存的宽高（此前编辑自定义分辨率连接时
  // 输入区保持 hidden 且宽高为空，用户无法查看/修改）。
  $("custom-res").classList.toggle("hidden", c.resolution_kind !== "custom");
  $("f-cw").value = c.custom_w > 0 ? String(c.custom_w) : "";
  $("f-ch").value = c.custom_h > 0 ? String(c.custom_h) : "";
  // F4 网关回显：token/密码不回填明文（placeholder 提示已保存）
  const gw = c.gateway || null;
  $("gateway-section").open = !!gw;
  $("f-gw-host").value = gw ? gw.host : "";
  $("f-gw-port").value = gw ? String(gw.port || 443) : "443";
  $("f-gw-tls").value = gw && !gw.use_tls ? "off" : "on";
  $("f-gw-auth").value = gw ? gw.auth : "token";
  $("f-gw-token").value = "";
  $("f-gw-token").placeholder = gw && gw.has_token ? "已保存（留空保持不变）" : "网关签发的短期 JWT（保存于本机）";
  $("f-gw-user").value = gw ? (gw.username || "") : "";
  $("f-gw-pass").value = "";
  $("f-gw-pass").placeholder = gw && gw.has_password ? "已保存（留空保持不变）" : "保存于本机，不上传";
  $("f-gw-resource").value = gw ? (gw.resource_id || "") : "";
  $("btn-cancel").classList.remove("hidden");
  $("f-password").placeholder = t("form.ph.passwordSaved");
}

function resetForm() {
  editingId = null;
  showErrors(null);
  $("form-title").textContent = t("form.new");
  for (const id of ["f-id", "f-name", "f-host", "f-username", "f-password", "f-group", "f-cw", "f-ch",
    "f-gw-host", "f-gw-token", "f-gw-user", "f-gw-pass", "f-gw-resource"]) $(id).value = "";
  $("f-port").value = "3389";
  $("f-gw-port").value = "443";
  $("f-gw-tls").value = "on";
  $("f-gw-auth").value = "token";
  $("f-gw-token").placeholder = "网关签发的短期 JWT（保存于本机）";
  $("f-gw-pass").placeholder = "保存于本机，不上传";
  $("gateway-section").open = false;
  $("f-resolution").value = "fit_window";
  // 新建/重置：恢复自定义分辨率输入区隐藏（2026-11 UI 修复）
  $("custom-res").classList.add("hidden");
  $("f-cw").value = "";
  $("f-ch").value = "";
  $("btn-cancel").classList.add("hidden");
  $("f-password").placeholder = t("form.ph.password");
}

async function saveCurrent() {
  const errs = validateForm();
  if (errs.length) { showErrors(errs); return; }
  showErrors(null);
  try {
    // F4 网关（可选）：折叠区展开且填了网关地址才提交
    let gateway = null;
    const gwHost = $("f-gw-host").value.trim();
    if ($("gateway-section").open && gwHost) {
      gateway = {
        host: gwHost,
        port: parseInt($("f-gw-port").value, 10) || 443,
        useTls: $("f-gw-tls").value !== "off",
        auth: $("f-gw-auth").value,
        token: $("f-gw-token").value,
        username: $("f-gw-user").value.trim(),
        password: $("f-gw-pass").value,
        resourceId: $("f-gw-resource").value.trim(),
      };
    }
    const view = await invoke("save_connection", {
      args: {
        id: editingId || "",
        name: $("f-name").value.trim(),
        host: $("f-host").value.trim(),
        port: parseInt($("f-port").value, 10),
        username: $("f-username").value.trim(),
        password: $("f-password").value,
        group: $("f-group").value.trim(),
        resolutionKind: $("f-resolution").value,
        customW: parseInt($("f-cw").value, 10) || 0,
        customH: parseInt($("f-ch").value, 10) || 0,
        gateway,
      },
    });
    editingId = view.id;
    $("f-id").value = view.id;
    $("form-title").textContent = t("form.editPrefix", { n: view.name });
    $("btn-cancel").classList.remove("hidden");
    flash(t("msg.saved"));
    await refresh();
  } catch (e) {
    flash(String(e), false);
  }
}

// 连接 = 直接发起远程会话（保存已在表单完成；密码后端从存储解密注入）
async function startSession(c) {
  flash(t("msg.connecting", { h: c.host, p: c.port }));
  // F-20261117-002：表单底部状态提示区（原 α 预览版说明行）
  setConnHint(`正在连接 ${c.host}:${c.port} …`, "busy");
  try {
    await openSession(c, (msg, ok) => { flash(msg, ok); refresh().catch(() => {}); });
    flash(c.rdp_ready ? t("msg.entered") : t("msg.enteredStub"));
    setConnHint(`已连接 ${c.host}（远程桌面窗口已打开）`, "ok");
  } catch (e) {
    setConnHint(`连接失败：${String(e)}`, "bad");
    flash(String(e), false);
  }
}

// ---- F-20261117-002/003：连接状态友好提示区（表单底部，唯一显示位） ----
// setConnHint = 连接生命周期状态（连接中/已连接/断开，常驻显示直到下一状态）；
// flash = 操作反馈（保存成功/删除/错误等，6s 后淡出——只在仍是该条消息时清除，
// 不冲掉连接生命周期状态）。R-20261117-003：原按钮右侧 #msg 已取消，全部走这里。
const connHintEl = () => document.getElementById("conn-hint");
let flashToken = 0;
export function setConnHint(text, level) {
  flashToken += 1; // 新状态到达 = 作废未过期的 flash 消息
  const el = connHintEl();
  if (!el) return;
  el.textContent = text || "\u00a0";
  el.classList.remove("hint-ok", "hint-bad", "hint-busy");
  if (level) el.classList.add(`hint-${level}`);
}
function flash(text, ok = true) {
  const token = ++flashToken;
  const el = connHintEl();
  if (!el) return;
  el.textContent = text;
  el.classList.remove("hint-ok", "hint-bad", "hint-busy");
  el.classList.add(ok ? "hint-ok" : "hint-bad");
  setTimeout(() => {
    if (flashToken === token) {
      el.textContent = "\u00a0";
      el.classList.remove("hint-ok", "hint-bad", "hint-busy");
    }
  }, 6000);
}
// rdp-view.js（独立模块）回调入口：原生窗口打开/断开事件同步到提示区
window.EasyRDPCallbacks = window.EasyRDPCallbacks || {};
window.EasyRDPCallbacks.setConnHint = setConnHint;

// TCP 连通性测试（保留：表单调试用，双击/连接按钮已直连画面）
async function tryConnect(c) {
  flash(`正在连接 ${c.host}:${c.port} …`);
  try {
    const ms = await invoke("test_connection", { host: c.host, port: c.port });
    setStatus(c.id, "ok");
    renderList();
    flash(`✓ ${c.host}:${c.port} 可达，TCP 握手 ${ms} ms。远程画面引擎（FreeRDP）将在后续版本接入。`);
  } catch (e) {
    setStatus(c.id, "fail");
    renderList();
    flash(String(e), false);
  }
}

// 事件绑定
$("btn-save").addEventListener("click", saveCurrent);
$("btn-cancel").addEventListener("click", resetForm);
$("btn-connect").addEventListener("click", async () => {
  // 连接按钮：先保存当前表单，再直接进入画面视图
  await saveCurrent();
  const c = connections.find((x) => x.id === editingId);
  if (!c) { flash(t("msg.pleaseSave"), false); return; }
  await startSession(c);
});
// 远程画面输入/工具条绑定（画布事件委托由 rdp-view.js 管理）
bindInput();
$("f-resolution").addEventListener("change", (e) => {
  $("custom-res").classList.toggle("hidden", e.target.value !== "custom");
});
// 侧栏：新建连接（切到表单空态）、搜索、新建分组、导出/导入 JSON
$("btn-new-conn").addEventListener("click", () => { resetForm(); $("f-name").focus(); });
$("conn-search").addEventListener("input", renderList);
$("btn-new-group").addEventListener("click", () => {
  // 为当前编辑/选中的连接设置分组：弹出输入，写入表单「分组」字段（保存时生效）
  const name = prompt(t("prompt.newGroup"));
  if (!name) return;
  $("f-group").value = name.trim();
  $("f-group").focus();
  flash(t("msg.groupFilled", { n: name.trim() }));
});
$("btn-export").addEventListener("click", async () => {
  try {
    const path = await dialog.save({
      defaultPath: "easyrdp-connections.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const n = await invoke("export_connections", { path });
    flash(t("msg.exportedAll", { n }));
  } catch (e) { flash(String(e), false); }
});
$("btn-import-json").addEventListener("click", async () => {
  try {
    const path = await dialog.open({
      multiple: false,
      directory: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const r = await invoke("import_connections", { path });
    flash(t("msg.importDone", { a: r[0], s: r[1] }));
    await refresh();
  } catch (e) { flash(String(e), false); }
});

// 导入 .rdp：弹出系统文件选择框，选定后自动导入并回填
$("btn-import").addEventListener("click", async () => {
  const path = await dialog.open({
    multiple: false,
    directory: false,
    filters: [{ name: "RDP 配置", extensions: ["rdp"] }],
  });
  if (!path) return;
  invoke("import_rdp", { path })
    .then(async (view) => { flash(t("msg.importedOne", { n: view.name })); editingId = view.id; await refresh(); loadToForm(view); })
    .catch((e) => flash(String(e), false));
});

// ---------- 使用反馈（加密提交到官网，协议见 src-tauri/src/feedback.rs） ----------
const fbModal = $("fb-modal");
$("btn-feedback").addEventListener("click", () => {
  $("fb-msg").textContent = "";
  fbModal.classList.remove("hidden");
  $("fb-content").focus();
});
$("fb-close").addEventListener("click", () => fbModal.classList.add("hidden"));
fbModal.addEventListener("click", (e) => { if (e.target === fbModal) fbModal.classList.add("hidden"); });
$("fb-count").addEventListener("click", () => {
  const el = $("fb-count-result");
  const raw = $("fb-content").value;
  const n = raw.length;               // 原始长度（与服务端 .trim 前统计口径一致展示）
  const cnt = raw.trim().length;      // 提交以 trim 后口径校验
  el.classList.remove("ok", "bad");
  if (cnt === 0) {
    el.textContent = t("fb.count.empty", { n });
    el.classList.add("bad");
  } else if (cnt < 5) {
    el.textContent = t("fb.count.few", { c: cnt, n, d: 5 - cnt });
    el.classList.add("bad");
  } else if (cnt > 2000) {
    el.textContent = t("fb.count.many", { c: cnt, n, o: cnt - 2000 });
    el.classList.add("bad");
  } else {
    el.textContent = t("fb.count.ok", { c: cnt });
    el.classList.add("ok");
  }
});
$("fb-content").addEventListener("input", () => {
  const el = $("fb-count-result");
  if (el) { el.textContent = ""; el.classList.remove("ok", "bad"); }
});
$("fb-submit").addEventListener("click", async () => {
  const btn = $("fb-submit");
  const content = $("fb-content").value.trim();
  if (content.length < 5) { fbMsg(t("fb.err.short"), false); return; }
  btn.disabled = true;
  fbMsg(t("fb.msg.submitting"), true);
  try {
    const res = await invoke("submit", {
      feedbackType: $("fb-type").value,
      contact: $("fb-contact").value.trim(),
      content,
    });
    fbMsg(res.message || (res.ok ? t("fb.msg.done") : t("fb.msg.fail")), res.ok);
    if (res.ok) { $("fb-content").value = ""; $("fb-contact").value = ""; }
  } catch (e) {
    fbMsg(t("fb.msg.fail") + ": " + String(e), false);
  } finally { btn.disabled = false; }
});
function fbMsg(text, ok) {
  const el = $("fb-msg");
  el.textContent = text;
  el.style.color = ok ? "var(--ok)" : "var(--warn)";
}

// ---------- R-012/R-011/R-013 启动序列：设置 -> 语言包 -> 主题 -> 首屏渲染 ----------
// （原启动直接 refresh()；现改为先载入设置与语言包，保证首屏文案即为用户所选语言）
(async () => {
  try {
    await bootstrapSettings();
  } catch (e) {
    console.warn("设置/语言初始化失败（按默认值继续）:", e);
  }
  // 设置框架（R-20261116-009）：绑定「⚙ 设置」按钮 / 弹窗交互
  initSettings();
  // 语言切换即时生效：静态文案由 i18n.applyI18n 重渲染；动态列表/表单在此重绘
  window.addEventListener("easyrdp-lang-changed", () => {
    renderList();
    const cur = connections.find((x) => x.id === editingId);
    if (cur) loadToForm(cur); else resetForm();
  });
  refresh().catch((e) => flash(String(e), false));
})();

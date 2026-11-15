// EasyRDP 连接中心前端。

import { openSession, closeSession, bindInput } from "./rdp-view.js";

const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;
const win = window.__TAURI__.window.getCurrentWindow();

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
const msgEl = $("msg");
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
  if (!$("f-host").value.trim()) errs.push("计算机（host）不能为空");
  const port = Number($("f-port").value.trim());
  if (!Number.isInteger(port) || port < 1 || port > 65535)
    errs.push(`端口非法：${$("f-port").value.trim() || "（空）"}（须为 1-65535 的整数）`);
  return errs;
}

function flash(text, ok = true) {
  msgEl.textContent = text;
  msgEl.style.color = ok ? "var(--ok)" : "var(--warn)";
  setTimeout(() => (msgEl.textContent = ""), 6000);
}

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
  for (const c of shown) {
    const g = (c.group || "").trim() || "未分组";
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(c);
  }
  const ordered = [...groups.entries()].sort(([a], [b]) => {
    if (a === "未分组") return 1;
    if (b === "未分组") return -1;
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
    item.title = "双击连接";
    const readyBadge = c.rdp_ready
      ? `<span class="badge ready" title="native 引擎已就绪，可发起远程连接">可连接</span>`
      : `<span class="badge pending" title="remote 画面引擎（FreeRDP）尚未接入">引擎接入中</span>`;
    item.innerHTML = `
      <div class="conn-name"><span class="status-dot ${st}" title="${st === "ok" ? "上次测试可达" : st === "fail" ? "上次测试不可达" : "未测试"}"></span>${esc(c.name)}${readyBadge}</div>
      <div class="conn-sub">${esc(c.host)}:${c.port}${c.username ? " · " + esc(c.username) : ""}</div>
      <div class="conn-ops">
        <button class="btn sm" data-act="edit">编辑</button>
        <button class="btn sm" data-act="exp" title="导出为 mstsc 兼容 .rdp 文件（不含密码）">导出.rdp</button>
        <button class="btn sm danger" data-act="del">删除</button>
      </div>`;
    item.addEventListener("click", (ev) => {
      const act = ev.target?.dataset?.act;
      if (act === "edit") { loadToForm(c); ev.stopPropagation(); return; }
      if (act === "exp") { exportRdp(c); ev.stopPropagation(); return; }
      if (act === "del") {
        if (confirm(`删除连接「${c.name}」？`)) {
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
    flash(`已导出「${c.name || c.host}」→ ${path} ✓`);
  } catch (e) { flash(String(e), false); }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (m) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[m]));
}

function loadToForm(c) {
  editingId = c.id;
  showErrors(null);
  $("form-title").textContent = `编辑连接 — ${c.name || c.host}`;
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
  $("btn-cancel").classList.remove("hidden");
  $("f-password").placeholder = "已保存（留空则保持不变，编辑后覆盖）";
}

function resetForm() {
  editingId = null;
  showErrors(null);
  $("form-title").textContent = "新建连接";
  for (const id of ["f-id", "f-name", "f-host", "f-username", "f-password", "f-group", "f-cw", "f-ch"]) $(id).value = "";
  $("f-port").value = "3389";
  $("f-resolution").value = "fit_window";
  // 新建/重置：恢复自定义分辨率输入区隐藏（2026-11 UI 修复）
  $("custom-res").classList.add("hidden");
  $("f-cw").value = "";
  $("f-ch").value = "";
  $("btn-cancel").classList.add("hidden");
  $("f-password").placeholder = "保存于本机，不上传";
}

async function saveCurrent() {
  const errs = validateForm();
  if (errs.length) { showErrors(errs); return; }
  showErrors(null);
  try {
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
      },
    });
    editingId = view.id;
    $("f-id").value = view.id;
    $("form-title").textContent = `编辑连接 — ${view.name}`;
    $("btn-cancel").classList.remove("hidden");
    flash("已保存 ✓");
    await refresh();
  } catch (e) {
    flash(String(e), false);
  }
}

// 连接 = 直接发起远程会话（保存已在表单完成；密码后端从存储解密注入）
async function startSession(c) {
  flash(`正在连接 ${c.host}:${c.port} …`);
  try {
    await openSession(c, (msg, ok) => { flash(msg, ok); refresh().catch(() => {}); });
    flash(`✓ 已进入远程画面${c.rdp_ready ? "" : "（stub 演示模式：引擎未编译，画面为本地测试图案）"}`);
  } catch (e) {
    flash(String(e), false);
  }
}

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
  if (!c) { flash("请先保存连接", false); return; }
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
  const name = prompt("新分组名称（将写入当前表单的「分组」字段，保存连接后生效）：");
  if (!name) return;
  $("f-group").value = name.trim();
  $("f-group").focus();
  flash(`分组已填入「${name.trim()}」，保存连接后生效`);
});
$("btn-export").addEventListener("click", async () => {
  try {
    const path = await dialog.save({
      defaultPath: "easyrdp-connections.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const n = await invoke("export_connections", { path });
    flash(`已导出 ${n} 条连接 ✓`);
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
    flash(`导入完成：新增 ${r[0]} 条，跳过（id 冲突）${r[1]} 条`);
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
    .then(async (view) => { flash(`已导入「${view.name}」✓`); editingId = view.id; await refresh(); loadToForm(view); })
    .catch((e) => flash(String(e), false));
});

refresh().catch((e) => flash(String(e), false));

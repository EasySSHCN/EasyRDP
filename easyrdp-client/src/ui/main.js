// EasyRDP 连接中心前端。

const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;

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
  countEl.textContent = connections.length;
  emptyEl.classList.toggle("hidden", connections.length > 0);
  const statusMap = loadStatus();
  for (const c of connections) {
    const st = statusMap[c.id]?.state || "unknown";
    const item = document.createElement("div");
    item.className = "conn-item" + (c.id === selectedId ? " selected" : "");
    item.title = "双击连接";
    item.innerHTML = `
      <div class="conn-name"><span class="status-dot ${st}" title="${st === "ok" ? "上次测试可达" : st === "fail" ? "上次测试不可达" : "未测试"}"></span>${esc(c.name)}</div>
      <div class="conn-sub">${esc(c.host)}:${c.port}${c.username ? " · " + esc(c.username) : ""}</div>
      <div class="conn-ops">
        <button class="btn sm" data-act="edit">编辑</button>
        <button class="btn sm danger" data-act="del">删除</button>
      </div>`;
    item.addEventListener("click", (ev) => {
      const act = ev.target?.dataset?.act;
      if (act === "edit") { loadToForm(c); ev.stopPropagation(); return; }
      if (act === "del") {
        if (confirm(`删除连接「${c.name}」？`)) {
          if (editingId === c.id) resetForm();  // 正在编辑该连接 => 重置表单
          clearStatus(c.id);
          invoke("delete_connection", { id: c.id }).then(refresh).catch((e) => flash(String(e), false));
        }
        ev.stopPropagation(); return;
      }
      selectedId = c.id;
      loadToForm(c);          // 单击即回填表单，便于查看/编辑
      renderList();
    });
    item.addEventListener("dblclick", async () => {
      loadToForm(c);          // 双击：自动填入表单 + 开始连接
      await tryConnect(c);
    });
    listEl.appendChild(item);
  }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (m) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[m]));
}

function loadToForm(c) {
  editingId = c.id;
  showErrors(null);
  $("form-title").textContent = "编辑连接";
  $("f-id").value = c.id;
  $("f-name").value = c.name;
  $("f-host").value = c.host;
  $("f-port").value = c.port;
  $("f-username").value = c.username;
  $("f-password").value = "";
  $("f-resolution").value = c.resolution_kind;
  $("btn-cancel").classList.remove("hidden");
  $("f-password").placeholder = "已保存（留空则保持不变，编辑后覆盖）";
}

function resetForm() {
  editingId = null;
  showErrors(null);
  $("form-title").textContent = "新建连接";
  for (const id of ["f-id", "f-name", "f-host", "f-username", "f-password", "f-cw", "f-ch"]) $(id).value = "";
  $("f-port").value = "3389";
  $("f-resolution").value = "fit_window";
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
        resolutionKind: $("f-resolution").value,
        customW: parseInt($("f-cw").value, 10) || 0,
        customH: parseInt($("f-ch").value, 10) || 0,
      },
    });
    editingId = view.id;
    $("f-id").value = view.id;
    $("form-title").textContent = "编辑连接";
    $("btn-cancel").classList.remove("hidden");
    flash("已保存 ✓");
    await refresh();
  } catch (e) {
    flash(String(e), false);
  }
}

// 连接 = 真实 TCP 连通性测试（远程画面引擎在后续版本接入）
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
$("btn-connect").addEventListener("click", () => {
  const c = connections.find((x) => x.id === editingId);
  if (!c) { flash("请先保存连接", false); return; }
  tryConnect(c);
});
$("f-resolution").addEventListener("change", (e) => {
  $("custom-res").classList.toggle("hidden", e.target.value !== "custom");
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

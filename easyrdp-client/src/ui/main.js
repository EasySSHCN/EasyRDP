// EasyRDP 连接中心前端。

const { invoke } = window.__TAURI__.core;

const $ = (id) => document.getElementById(id);
const listEl = $("conn-list");
const emptyEl = $("conn-empty");
const countEl = $("conn-count");
const msgEl = $("msg");
let connections = [];
let editingId = null;
let selectedId = null;

function flash(text, ok = true) {
  msgEl.textContent = text;
  msgEl.style.color = ok ? "var(--ok)" : "var(--bad)";
  setTimeout(() => (msgEl.textContent = ""), 3000);
}

async function refresh() {
  connections = await invoke("list_connections");
  renderList();
}

function renderList() {
  listEl.innerHTML = "";
  countEl.textContent = connections.length;
  emptyEl.classList.toggle("hidden", connections.length > 0);
  for (const c of connections) {
    const item = document.createElement("div");
    item.className = "conn-item" + (c.id === selectedId ? " selected" : "");
    item.innerHTML = `
      <div class="conn-name">${esc(c.name)}</div>
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
          invoke("delete_connection", { id: c.id }).then(refresh).catch((e) => flash(String(e), false));
        }
        ev.stopPropagation(); return;
      }
      selectedId = c.id;
      renderList();
    });
    item.addEventListener("dblclick", () => tryConnect(c));
    listEl.appendChild(item);
  }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (m) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[m]));
}

function loadToForm(c) {
  editingId = c.id;
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
  $("form-title").textContent = "新建连接";
  for (const id of ["f-id", "f-name", "f-host", "f-username", "f-password", "f-cw", "f-ch"]) $(id).value = "";
  $("f-port").value = "3389";
  $("f-resolution").value = "fit_window";
  $("btn-cancel").classList.add("hidden");
  $("f-password").placeholder = "保存于本机，不上传";
}

async function saveCurrent() {
  try {
    const view = await invoke("save_connection", {
      args: {
        id: editingId || "",
        name: $("f-name").value.trim(),
        host: $("f-host").value.trim(),
        port: parseInt($("f-port").value, 10) || 3389,
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

async function tryConnect(c) {
  flash(`「${c.name}」远程画面引擎将在后续版本提供（FreeRDP 集成中）`, false);
}

// 事件绑定
$("btn-save").addEventListener("click", saveCurrent);
$("btn-cancel").addEventListener("click", resetForm);
$("btn-connect").addEventListener("click", () => {
  if (!editingId) { flash("请先保存连接", false); return; }
  const c = connections.find((x) => x.id === editingId);
  if (c) tryConnect(c);
});
$("f-resolution").addEventListener("change", (e) => {
  $("custom-res").classList.toggle("hidden", e.target.value !== "custom");
});
$("btn-import").addEventListener("click", () => {
  const path = prompt("输入 mstsc 导出的 .rdp 文件绝对路径：");
  if (!path) return;
  invoke("import_rdp", { path })
    .then(async (view) => { flash(`已导入「${view.name}」✓`); editingId = view.id; await refresh(); loadToForm(view); })
    .catch((e) => flash(String(e), false));
});

refresh().catch((e) => flash(String(e), false));

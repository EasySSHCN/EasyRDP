// EasyRDP 连接中心前端。

import { openSession, closeSession, bindInput } from "./rdp-view.js";
import { bootstrapSettings, initSettings, getSettings } from "./settings.js?v=2026111701";
import { t, applyI18n } from "./i18n.js?v=2026111602";
import { initManualTooltips } from "./tooltip.js?v=2026111901";

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
  try { groups = await invoke("list_groups"); } catch { groups = []; }
  renderList();
}

// F6-V2（R-20261118-015）：Git 同步从远端恢复后刷新连接/分组列表
window.addEventListener("easyrdp-sync-pulled", () => { refresh(); });

// ==================== F1-V1 收藏与分组增强（R-20261118-007） ====================
// 预设 8 色色板（与 crates/easyrdp-core/src/groups.rs PALETTE 保持一致）
const PALETTE = ["#ef4444", "#f59e0b", "#22c55e", "#06b6d4", "#3b82f6", "#8b5cf6", "#ec4899", "#64748b"];
const MAX_GROUP_DEPTH = 3;                       // 分组最多 3 层（1 = 顶层）
const GROUPS_COLLAPSE_KEY = "easyrdp-collapsed-groups";
let groups = [];                                 // {id,name,parent,color,order}
let collapsedGroups = (() => {
  try { return new Set(JSON.parse(localStorage.getItem(GROUPS_COLLAPSE_KEY) || "[]")); }
  catch { return new Set(); }
})();
let dragPayload = null;                          // {kind: "conn"|"fav"|"group", id}
let editingMeta = null;                          // 编辑表单携带的列表元数据

function saveCollapsed() {
  localStorage.setItem(GROUPS_COLLAPSE_KEY, JSON.stringify([...collapsedGroups]));
}

function colorOf(gid) {
  const g = groups.find((x) => x.id === gid);
  return g && g.color ? g.color : null;
}

function groupChildren(pid) {
  return groups
    .filter((g) => (g.parent || null) === pid)
    .sort((a, b) => (a.order || 0) - (b.order || 0) || a.name.localeCompare(b.name, "zh-CN"));
}

function groupDepth(g) {
  let d = 1;
  let cur = g;
  const seen = new Set([cur.id]);
  while (cur && cur.parent) {
    if (seen.has(cur.parent)) return 99; // 防御：成环按超深处理
    seen.add(cur.parent);
    cur = groups.find((x) => x.id === cur.parent);
    d += 1;
  }
  return d;
}

function isDescendantGroup(gid, ancestorId) {
  let cur = groups.find((x) => x.id === gid);
  for (let i = 0; cur && i < groups.length; i += 1) {
    if (cur.parent === ancestorId) return true;
    cur = cur.parent ? groups.find((x) => x.id === cur.parent) : null;
  }
  return false;
}

async function saveGroups() {
  try {
    await invoke("save_groups", { groups });
  } catch (e) {
    flash(String(e), false);
  }
  await refresh();
}

async function createGroup(parentId = null) {
  const name = prompt(parentId ? "子分组名称：" : "分组名称：");
  if (!name || !name.trim()) return;
  if (parentId && !groups.find((x) => x.id === parentId)) { flash("目标分组不存在", false); return; }
  const parent = parentId ? groups.find((x) => x.id === parentId) : null;
  if (parent && groupDepth(parent) >= MAX_GROUP_DEPTH) {
    flash(`已达到 ${MAX_GROUP_DEPTH} 层嵌套上限，请选择更上层的分组`, false);
    return;
  }
  const order = groups
    .filter((g) => (g.parent || null) === parentId)
    .reduce((m, g) => Math.max(m, g.order || 0), 0) + 1;
  groups.push({
    id: `grp-${Date.now()}-${Math.floor(Math.random() * 1e4)}`,
    name: name.trim(),
    parent: parentId,
    color: null,
    order,
  });
  await saveGroups();
  flash(`分组「${name.trim()}」已创建`);
}

async function toggleFavorite(c, fav) {
  try {
    const favs = connections.filter((x) => x.favorite);
    const favOrder = fav ? favs.reduce((m, x) => Math.max(m, x.fav_order || 0), 0) + 1 : 0;
    await invoke("update_connection_meta", { id: c.id, favorite: fav, favOrder });
    await refresh();
    flash(fav ? `已收藏「${c.name || c.host}」并置顶` : "已取消收藏");
  } catch (e) { flash(String(e), false); }
}

// 把一组连接按数组顺序写回 order 字段（批量 update_connection_meta）
async function persistOrders(reorderedIds) {
  for (let i = 0; i < reorderedIds.length; i += 1) {
    try {
      await invoke("update_connection_meta", { id: reorderedIds[i], order: i });
    } catch (e) { flash(String(e), false); break; }
  }
  await refresh();
}

// 移动/归组：设置 group_id 并重排该组内顺序（插到 beforeId 之前；beforeId 空 = 追加末尾）
async function moveConn(connId, groupId, beforeId) {
  const conn = connections.find((x) => x.id === connId);
  if (!conn) return;
  if (beforeId === connId) return;
  conn.group_id = groupId || "";
  const inGroup = connections
    .filter((x) => (x.group_id || "") === (groupId || ""))
    .sort((a, b) => (a.order || 0) - (b.order || 0));
  const rest = inGroup.filter((x) => x.id !== connId);
  const idx = beforeId ? rest.findIndex((x) => x.id === beforeId) : -1;
  if (idx >= 0) rest.splice(idx, 0, conn); else rest.push(conn);
  await persistOrders(rest.map((x) => x.id));
}

async function persistFavOrder(favIds) {
  for (let i = 0; i < favIds.length; i += 1) {
    try {
      await invoke("update_connection_meta", { id: favIds[i], favOrder: i });
    } catch (e) { flash(String(e), false); break; }
  }
  await refresh();
}

// ---- 拖拽（HTML5 DnD；WebView2 支持）----
let lastDropEl = null;
function clearDragOver() {
  if (lastDropEl) { lastDropEl.classList.remove("drag-over"); lastDropEl = null; }
}
function markDragOver(el) {
  if (lastDropEl !== el) { clearDragOver(); el.classList.add("drag-over"); lastDropEl = el; }
}

async function handleDrop(target) {
  if (!target || !dragPayload) return;
  const { kind, id } = dragPayload;
  if (target.classList.contains("conn-item")) {
    if (kind === "conn") await moveConn(id, target.dataset.gid || "", target.dataset.cid);
    return;
  }
  if (target.classList.contains("tree-group")) {
    const gid = target.dataset.gid;
    if (kind === "conn") { await moveConn(id, gid, null); return; }
    if (kind === "group") {
      const drag = groups.find((x) => x.id === id);
      const tgt = groups.find((x) => x.id === gid);
      if (!drag || !tgt || id === gid) return;
      if (isDescendantGroup(gid, id)) {
        flash("已忽略：目标分组位于被拖动分组的子级中", false);
        return;
      }
      if ((drag.parent || null) === (tgt.parent || null)) {
        // 同级：插到目标之前（组内重排）
        const siblings = groupChildren(drag.parent || null).filter((x) => x.id !== id);
        const idx = siblings.findIndex((x) => x.id === gid);
        if (idx >= 0) siblings.splice(idx, 0, drag); else siblings.push(drag);
        siblings.forEach((g, i) => { g.order = i; });
        await saveGroups();
      } else {
        // 跨层：挂为目标子分组（深度校验）
        if (groupDepth(tgt) + 1 > MAX_GROUP_DEPTH) {
          flash(`已达到 ${MAX_GROUP_DEPTH} 层嵌套上限`, false);
          return;
        }
        drag.parent = gid;
        drag.order = groups.filter((g) => g.parent === gid).reduce((m, g) => Math.max(m, g.order || 0), 0) + 1;
        await saveGroups();
        flash(`「${drag.name}」已移入「${tgt.name}」`);
      }
    }
    return;
  }
  if (target.classList.contains("legacy-head") || target.classList.contains("ungrouped-head")) {
    if (kind === "conn") await moveConn(id, "", null);
  }
}

// ---- 分组颜色弹层（8 色色板 + 清除）----
let colorPopEl = null;
function hideColorPop() {
  if (colorPopEl) { colorPopEl.remove(); colorPopEl = null; }
}
function showColorPop(gid, anchor) {
  hideColorPop();
  const g = groups.find((x) => x.id === gid);
  if (!g) return;
  colorPopEl = document.createElement("div");
  colorPopEl.className = "color-pop";
  for (const col of PALETTE) {
    const sw = document.createElement("button");
    sw.type = "button";
    sw.className = "swatch" + (g.color === col ? " on" : "");
    sw.style.background = col;
    sw.title = col;
    sw.addEventListener("click", async () => {
      g.color = col;
      hideColorPop();
      await saveGroups();
    });
    colorPopEl.appendChild(sw);
  }
  const clear = document.createElement("button");
  clear.type = "button";
  clear.className = "swatch none";
  clear.textContent = "✕";
  clear.title = "无色";
  clear.addEventListener("click", async () => {
    g.color = null;
    hideColorPop();
    await saveGroups();
  });
  colorPopEl.appendChild(clear);
  document.body.appendChild(colorPopEl);
  const r = anchor.getBoundingClientRect();
  colorPopEl.style.left = `${Math.max(8, r.left - 40)}px`;
  colorPopEl.style.top = `${r.bottom + 6}px`;
}
document.addEventListener("click", (e) => {
  if (colorPopEl && !colorPopEl.contains(e.target) && !(e.target.closest && e.target.closest('[data-gact="color"]'))) {
    hideColorPop();
  }
});

// ---- 收藏置顶栏 ----
function renderFavBar(q) {
  const bar = $("fav-bar");
  if (!bar) return;
  const favs = connections
    .filter((c) => c.favorite && connVisible(c, q))
    .sort((a, b) => (a.fav_order || 0) - (b.fav_order || 0));
  bar.classList.toggle("hidden", favs.length === 0);
  bar.innerHTML = "";
  for (const c of favs) {
    const chip = document.createElement("span");
    chip.className = "fav-chip";
    chip.draggable = true;
    chip.dataset.cid = c.id;
    const col = colorOf(c.group_id);
    if (col) chip.style.borderLeft = `3px solid ${col}`;
    chip.title = `${c.name || c.host} · ${c.host}:${c.port}（点击连接，拖拽排序）`;
    chip.innerHTML =
      `<span class="fav-star">★</span>` +
      `<span class="fav-name">${esc(c.name || c.host)}</span>` +
      `<span class="fav-x" data-fx="${esc(c.id)}" title="取消收藏">✕</span>`;
    chip.addEventListener("mouseenter", () => scheduleSnapshotPreview(chip, c.id));
    chip.addEventListener("mouseleave", cancelSnapshotPreview);
    chip.addEventListener("dragstart", (e) => {
      dragPayload = { kind: "fav", id: c.id };
      e.dataTransfer.effectAllowed = "move";
      try { e.dataTransfer.setData("text/plain", c.id); } catch { /* ignore */ }
    });
    chip.addEventListener("click", (ev) => {
      if (ev.target.dataset && ev.target.dataset.fx) {
        toggleFavorite(c, false);
        ev.stopPropagation();
        return;
      }
      startSession(c); // 点击即连
    });
    bar.appendChild(chip);
  }
}

function connVisible(c, q) {
  return !q ||
    (c.name || "").toLowerCase().includes(q) ||
    (c.host || "").toLowerCase().includes(q) ||
    (c.username || "").toLowerCase().includes(q);
}

function renderList() {
  cancelSnapshotPreview(); // 列表重渲染时收起快照浮层（防悬挂）
  renderFavBar(($("conn-search").value || "").trim().toLowerCase());
  listEl.innerHTML = "";
  // 搜索过滤（前端过滤：按名称/主机/用户名，大小写不敏感）
  const q = ($("conn-search").value || "").trim().toLowerCase();
  const shown = connections.filter((c) => connVisible(c, q));
  countEl.textContent = shown.length;
  emptyEl.classList.toggle("hidden", shown.length > 0);
  if (!connections.length) return;
  const statusMap = loadStatus();

  const connsOf = (gid) => shown
    .filter((c) => (c.group_id || "") === gid)
    .sort((a, b) => (a.order || 0) - (b.order || 0));

  // 旧数据兼容：group 字符串未入分组树的，渲染为顶层虚拟分组
  const legacyNames = [...new Set(shown
    .filter((c) => !(c.group_id || "") && (c.group || "").trim())
    .map((c) => c.group.trim()))];
  const treeNames = new Set(groups.map((g) => g.name));

  const renderGroupNode = (g, container, depth) => {
    const kids = groupChildren(g.id);
    const items = connsOf(g.id);
    const isCollapsed = collapsedGroups.has(g.id);
    const node = document.createElement("div");
    node.className = "tree-group";
    node.dataset.gid = g.id;
    node.draggable = true;
    node.style.paddingLeft = `${4 + depth * 14}px`;
    const dot = g.color
      ? `<span class="g-dot" style="background:${esc(g.color)}"></span>`
      : `<span class="g-dot none"></span>`;
    node.innerHTML =
      `<span class="caret">${isCollapsed ? "▸" : "▾"}</span>${dot}` +
      `<span class="g-name">${esc(g.name)}</span>` +
      `<span class="g-count">（${items.length + kids.length}）</span>` +
      `<span class="g-ops">` +
      `<button class="btn sm g-btn" data-gact="child" title="新建子分组">＋</button>` +
      `<button class="btn sm g-btn" data-gact="color" title="分组颜色">🎨</button>` +
      `</span>`;
    node.addEventListener("click", (ev) => {
      const act = ev.target.dataset && ev.target.dataset.gact;
      if (act === "child") { createGroup(g.id); ev.stopPropagation(); return; }
      if (act === "color") { showColorPop(g.id, ev.target); ev.stopPropagation(); return; }
      if (ev.target.closest(".g-ops")) return;
      // 点击 = 折叠/展开
      if (collapsedGroups.has(g.id)) collapsedGroups.delete(g.id);
      else collapsedGroups.add(g.id);
      saveCollapsed();
      renderList();
    });
    node.addEventListener("dragstart", (e) => {
      dragPayload = { kind: "group", id: g.id };
      e.dataTransfer.effectAllowed = "move";
      try { e.dataTransfer.setData("text/plain", g.id); } catch { /* ignore */ }
    });
    container.appendChild(node);
    if (isCollapsed) return;
    const box = document.createElement("div");
    box.className = "group-children";
    for (const k of kids) renderGroupNode(k, box, depth + 1);
    for (const c of items) renderConnItem(c, statusMap, box, depth + 1);
    container.appendChild(box);
  };

  // 顶层分组树
  for (const g of groupChildren(null)) renderGroupNode(g, listEl, 0);

  // 遗留字符串分组（旧 connections.json 兼容展示）
  for (const name of legacyNames) {
    if (treeNames.has(name)) continue;
    const head = document.createElement("div");
    head.className = "group-head legacy-head";
    const items = shown.filter((c) => !(c.group_id || "") && (c.group || "").trim() === name);
    head.textContent = `▾ ${name}（${items.length}）`;
    head.title = "旧版字符串分组；把连接拖入分组树后可移除";
    listEl.appendChild(head);
    for (const c of items) renderConnItem(c, statusMap, listEl, 0);
  }

  // 未分组（排最后）
  const ungroupedItems = shown.filter((c) => !(c.group_id || "") && !(c.group || "").trim());
  if (ungroupedItems.length) {
    const head = document.createElement("div");
    head.className = "group-head ungrouped-head";
    head.textContent = `▾ ${t("group.ungrouped")}（${ungroupedItems.length}）`;
    listEl.appendChild(head);
    for (const c of ungroupedItems) renderConnItem(c, statusMap, listEl, 0);
  }
}

// 列表区拖放委托（连接重排 / 归组 / 分组嵌套重排）
listEl.addEventListener("dragover", (e) => {
  if (!dragPayload) return;
  const t = e.target.closest(".conn-item, .tree-group, .legacy-head, .ungrouped-head");
  if (!t) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = "move";
  markDragOver(t);
});
listEl.addEventListener("drop", (e) => {
  if (!dragPayload) return;
  e.preventDefault();
  const t = e.target.closest(".conn-item, .tree-group, .legacy-head, .ungrouped-head");
  const payload = dragPayload;
  dragPayload = null;
  clearDragOver();
  if (t) handleDropWith(t, payload);
});
// 收藏栏拖放：收藏排序 + 非收藏连接拖入即收藏
$("fav-bar").addEventListener("dragover", (e) => {
  if (!dragPayload) return;
  const t = e.target.closest(".fav-chip") || e.currentTarget;
  e.preventDefault();
  e.dataTransfer.dropEffect = "move";
  markDragOver(t.classList.contains("fav-chip") ? t : e.currentTarget);
});
$("fav-bar").addEventListener("drop", async (e) => {
  if (!dragPayload) return;
  e.preventDefault();
  const { kind, id } = dragPayload;
  dragPayload = null;
  clearDragOver();
  if (kind === "conn") {
    const c = connections.find((x) => x.id === id);
    if (c && !c.favorite) await toggleFavorite(c, true);
    return;
  }
  if (kind === "fav") {
    const favs = connections
      .filter((x) => x.favorite)
      .sort((a, b) => (a.fav_order || 0) - (b.fav_order || 0));
    const rest = favs.filter((x) => x.id !== id);
    const chip = e.target.closest && e.target.closest(".fav-chip");
    const idx = chip ? rest.findIndex((x) => x.id === chip.dataset.cid) : -1;
    if (idx >= 0) rest.splice(idx, 0, { id }); else rest.push({ id });
    await persistFavOrder(rest.map((x) => x.id));
  }
});
document.addEventListener("dragend", () => {
  dragPayload = null;
  clearDragOver();
});

// handleDrop 的依赖注入版（drop 委托里先把 dragPayload 置空再调用）
async function handleDropWith(target, payload) {
  const saved = dragPayload;
  dragPayload = payload;
  await handleDrop(target);
  dragPayload = saved;
}

function renderConnItem(c, statusMap, container, depth) {
    const st = statusMap[c.id]?.state || "unknown";
    const item = document.createElement("div");
    item.className = "conn-item" + (c.id === selectedId ? " selected" : "");
    item.dataset.cid = c.id;
    item.dataset.gid = c.group_id || "";
    item.draggable = true;
    item.title = t("conn.title.dblClick");
    if (depth) item.style.marginLeft = `${depth * 14}px`;
    const col = colorOf(c.group_id);
    if (col) item.style.borderLeft = `3px solid ${col}`;
    const readyBadge = c.rdp_ready
      ? `<span class="badge ready" title="${esc(t("conn.badge.readyTip"))}">${esc(t("conn.badge.ready"))}</span>`
      : `<span class="badge pending" title="${esc(t("conn.badge.pendingTip"))}">${esc(t("conn.badge.pending"))}</span>`;
    const dotTitle = st === "ok" ? t("conn.dot.ok") : st === "fail" ? t("conn.dot.fail") : t("conn.dot.unknown");
    item.innerHTML = `
      <div class="conn-name"><span class="status-dot ${st}" title="${esc(dotTitle)}"></span>${esc(c.name)}${readyBadge}</div>
      <div class="conn-sub">${esc(c.host)}:${c.port}${c.username ? " · " + esc(c.username) : ""}</div>
      <div class="conn-ops">
        <button class="btn sm" data-act="fav" title="${c.favorite ? "取消收藏" : "收藏置顶"}">${c.favorite ? "★" : "☆"}</button>
        <button class="btn sm" data-act="edit">${esc(t("conn.edit"))}</button>
        <button class="btn sm" data-act="exp" title="${esc(t("conn.title.export"))}">${esc(t("conn.export"))}</button>
        <button class="btn sm" data-act="tpl" title="${esc(t("conn.title.saveTpl"))}">${esc(t("conn.saveTpl"))}</button>
        <button class="btn sm danger" data-act="del">${esc(t("conn.delete"))}</button>
      </div>`;
    item.addEventListener("dragstart", (e) => {
      dragPayload = { kind: "conn", id: c.id };
      e.dataTransfer.effectAllowed = "move";
      try { e.dataTransfer.setData("text/plain", c.id); } catch { /* ignore */ }
    });
    item.addEventListener("click", (ev) => {
      const act = ev.target?.dataset?.act;
      if (act === "fav") { toggleFavorite(c, !c.favorite); ev.stopPropagation(); return; }
      if (act === "edit") { loadToForm(c); ev.stopPropagation(); return; }
      if (act === "exp") { exportRdp(c); ev.stopPropagation(); return; }
      if (act === "tpl") { saveAsTemplate(c); ev.stopPropagation(); return; }
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
    // F1-V2 R-20261118-013 会话快照预览：hover 500ms 防抖 -> 后端读快照 -> 浮层
    item.addEventListener("mouseenter", () => scheduleSnapshotPreview(item, c.id));
    item.addEventListener("mouseleave", cancelSnapshotPreview);
    container.appendChild(item);
}

// ==================== F1-V2 R-20261118-013 会话快照预览 ====================
// 卡片 hover 500ms 防抖 -> invoke get_snapshot -> 卡片上方浮窗（图片 + 主机名
// + 最近会话时间戳）。隐私开关 autoSnapshotPreview 关闭时不请求不显示。

let snapHoverTimer = null;
let snapPop = null;
let snapHoverSeq = 0;

function cancelSnapshotPreview() {
  if (snapHoverTimer) { clearTimeout(snapHoverTimer); snapHoverTimer = null; }
  snapHoverSeq++;
  if (snapPop) { snapPop.remove(); snapPop = null; }
}

function fmtSnapTime(ms) {
  if (!ms) return "";
  const d = new Date(Number(ms));
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

function positionSnapshotPop(item) {
  if (!snapPop) return;
  const r = item.getBoundingClientRect();
  snapPop.style.visibility = "hidden";
  snapPop.style.display = "block";
  const pw = snapPop.offsetWidth, ph = snapPop.offsetHeight;
  let left = Math.max(8, Math.min(r.left, window.innerWidth - pw - 8));
  let top = r.top - ph - 8;
  if (top < 8) top = r.bottom + 8; // 卡片贴顶时改浮在下方
  snapPop.style.left = `${left}px`;
  snapPop.style.top = `${top}px`;
  snapPop.style.visibility = "visible";
}

function scheduleSnapshotPreview(item, profileId) {
  cancelSnapshotPreview();
  // 隐私开关：关闭时不请求（后端也已停止捕获）
  if (getSettings().autoSnapshotPreview === false) return;
  const seq = ++snapHoverSeq;
  snapHoverTimer = setTimeout(async () => {
    snapHoverTimer = null;
    if (seq !== snapHoverSeq) return;
    let data = null;
    try { data = await invoke("get_snapshot", { profileId }); } catch { return; }
    if (seq !== snapHoverSeq || !item.isConnected) return;
    // 修复：此处不能调用 cancelSnapshotPreview（它会自增 snapHoverSeq，
    // 导致下方 append 后的 seq 校验永真、浮层刚挂上就被删）；只做定点清理
    if (snapPop) snapPop.remove();
    snapPop = document.createElement("div");
    snapPop.id = "snapshot-pop";
    snapPop.className = "snapshot-pop";
    const host = (connections.find((x) => x.id === profileId) || {}).host || profileId;
    if (data && data.pngBase64) {
      snapPop.innerHTML =
        `<img class="snap-img" src="data:image/png;base64,${data.pngBase64}" alt="snapshot" />` +
        `<div class="snap-meta"><span class="snap-host">${esc(host)}</span>` +
        `<span class="snap-time">${esc(t("snap.title.lastSession"))}: ${esc(fmtSnapTime(data.capturedAtMs))}</span></div>`;
    } else {
      snapPop.innerHTML =
        `<div class="snap-empty">${esc(t("snap.none"))}</div>` +
        `<div class="snap-meta"><span class="snap-host">${esc(host)}</span></div>`;
    }
    document.body.appendChild(snapPop);
    // 鼠标已提前离开 / 列表已重渲染则不显示（seq 自增已移除，此处仅 isConnected 需要复检）
    if (!item.isConnected) { cancelSnapshotPreview(); return; }
    positionSnapshotPop(item);
    snapPop.addEventListener("mouseenter", cancelSnapshotPreview); // 移入浮层即隐藏
  }, 500);
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
  // F1-V1：编辑保存时回传列表元数据，避免表单保存清掉收藏/排序/树归属
  editingMeta = {
    favorite: !!c.favorite,
    fav_order: c.fav_order || 0,
    group_id: c.group_id || "",
    order: c.order || 0,
  };
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
  // F3-V1：回显显示器选择 / 跨屏（fullscreen 模式附加区可见）
  $("multimon-opts").classList.toggle("hidden", c.resolution_kind !== "fullscreen");
  $("f-monitor").value = (c.monitor != null) ? String(c.monitor) : "";
  $("f-multimon").checked = !!c.multi_monitor;
  $("f-monitor").disabled = !!c.multi_monitor;
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
  $("f-gw-self-signed").checked = !!(gw && gw.allow_self_signed);
  // F4-V2：备用网关列表回显 + 清空上次探测结果
  renderGwFallbacks(gw && gw.fallbacks ? gw.fallbacks : []);
  $("gw-health-results").textContent = "";
  // 端口敲门（P0-P2）回显：SPA 密钥不回填明文（placeholder 提示已保存）
  const knock = c.knock || null;
  $("knock-section").open = !!knock;
  $("f-precmd").value = c.pre_connect_command || "";
  loadKnockToForm(knock);
  $("btn-cancel").classList.remove("hidden");
  $("f-password").placeholder = t("form.ph.passwordSaved");
}

function resetForm() {
  editingId = null;
  editingMeta = null;
  showErrors(null);
  $("form-title").textContent = t("form.new");
  for (const id of ["f-id", "f-name", "f-host", "f-username", "f-password", "f-group", "f-cw", "f-ch",
    "f-gw-host", "f-gw-token", "f-gw-user", "f-gw-pass", "f-gw-resource"]) $(id).value = "";
  $("f-port").value = "3389";
  $("f-gw-port").value = "443";
  $("f-gw-tls").value = "on";
  $("f-gw-auth").value = "token";
  $("f-gw-self-signed").checked = false;
  $("f-gw-token").placeholder = "网关签发的短期 JWT（保存于本机）";
  $("f-gw-pass").placeholder = "保存于本机，不上传";
  // F4-V2：备用网关列表 / 探测结果复位
  renderGwFallbacks([]);
  $("gw-health-results").textContent = "";
  $("gateway-section").open = false;
  // 端口敲门（P0-P2）复位：命令清空 + 敲门默认关闭
  $("knock-section").open = false;
  $("f-precmd").value = "";
  loadKnockToForm(null);
  $("f-resolution").value = "fit_window";
  // 新建/重置：恢复自定义分辨率输入区隐藏（2026-11 UI 修复）
  $("custom-res").classList.add("hidden");
  $("f-cw").value = "";
  $("f-ch").value = "";
  // F3-V1：显示器选择/跨屏复位（主显示器、不跨屏）
  $("multimon-opts").classList.add("hidden");
  $("f-monitor").value = "";
  $("f-monitor").disabled = false;
  $("f-multimon").checked = false;
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
        allowSelfSigned: $("f-gw-self-signed").checked,
        // F4-V2：备用网关整表提交（凭据共享主网关，后端不重复存储）
        fallbacks: collectGwFallbacks(),
      };
    }
    // 端口敲门（P0-P2）：折叠区展开才提交（后端未提交时沿用已保存配置）
    let knock = null;
    if ($("knock-section").open) {
      knock = {
        enabled: $("f-knock-enabled").checked,
        knockType: $("f-knock-type").value,
        sequence: parseSeqPorts($("f-knock-seq").value),
        protocol: $("f-knock-proto").value,
        seqTimeout: parseInt($("f-knock-seqtimeout").value, 10) || 10,
        delayMs: parseInt($("f-knock-delayms").value, 10) || 0,
        spaPort: parseInt($("f-knock-spaport").value, 10) || 62201,
        key: $("f-knock-key").value,
        hmacKey: $("f-knock-hmac").value,
        openPorts: $("f-knock-openports").value.trim() || "tcp/3389",
        fwTimeout: parseInt($("f-knock-fwtimeout").value, 10) || 30,
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
        // F3-V1 多显示器优化：全屏目标显示器 / 跨所有显示器
        monitor: $("f-monitor").value !== "" ? parseInt($("f-monitor").value, 10) : null,
        multiMonitor: $("f-multimon").checked,
        gateway,
        // 端口敲门（P0-P2）：连接前命令 + 敲门配置
        preConnectCommand: $("f-precmd").value.trim(),
        knock,
        // F1-V1：编辑已有连接时回传列表元数据（新建 = 全零值）
        favorite: editingMeta ? editingMeta.favorite : false,
        favOrder: editingMeta ? editingMeta.fav_order : 0,
        groupId: editingMeta ? editingMeta.group_id : "",
        order: editingMeta ? editingMeta.order : 0,
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
// F4-V1：网关 OTP 二次认证——连接错误串含 "otp_required" 时弹码，经
// set_gateway_otp 一次性码槽写入后端后重试（至多一次自动重试）。
async function startSession(c) {
  flash(t("msg.connecting", { h: c.host, p: c.port }));
  // 敲门状态可视化（P3）：未敲 → 敲门中 → 已开门（倒计时）
  const knockEnabled = !!(c.knock && c.knock.enabled);
  const knockWindow = knockEnabled
    ? (c.knock.knock_type === "spa"
      ? (c.knock.fw_timeout || 30)
      : ((c.knock.seq_timeout || 10) + 10))
    : 0;
  setConnHint(
    knockEnabled
      ? `敲门中 ${c.host}（${c.knock.knock_type === "spa" ? "SPA" : "序列敲门"}）…`
      : `正在连接 ${c.host}:${c.port} …`,
    "busy",
  );
  let countdownTimer = null;
  const stopCountdown = () => { if (countdownTimer) { clearInterval(countdownTimer); countdownTimer = null; } };
  try {
    await openSession(c, (msg, ok) => { flash(msg, ok); refresh().catch(() => {}); });
    flash(c.rdp_ready ? t("msg.entered") : t("msg.enteredStub"));
    if (knockEnabled) {
      // 已开门 + 倒计时（近似开门窗口，每秒刷新直至归零）
      let left = knockWindow;
      const show = () => setConnHint(`已开门（剩余 ${left}s，期内重连不重复敲门）· ${c.host}`, "ok");
      show();
      countdownTimer = setInterval(() => {
        left -= 1;
        if (left <= 0) { stopCountdown(); setConnHint(`已连接 ${c.host}（开门窗口已过，重连将重新敲门）`, "ok"); }
        else show();
      }, 1000);
    } else {
      setConnHint(`已连接 ${c.host}（远程桌面窗口已打开）`, "ok");
    }
  } catch (e) {
    stopCountdown();
    const msg = String(e);
    if (msg.includes("otp_required") && !c._otpRetried) {
      const code = prompt("网关要求 OTP 二次认证，请输入 6 位验证码：");
      if (code && code.trim()) {
        try {
          await invoke("set_gateway_otp", { code: code.trim() });
        } catch (e2) {
          setConnHint(`OTP 提交失败：${String(e2)}`, "bad");
          flash(String(e2), false);
          return;
        }
        setConnHint(`正在以 OTP 重试连接 ${c.host}:${c.port} …`, "busy");
        return startSession({ ...c, _otpRetried: true });
      }
    }
    setConnHint(`连接失败：${knockErrorText(msg)}`, "bad");
    flash(knockErrorText(msg), false);
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

// ==================== 端口敲门（P0-P2）UI ====================

// 解析逗号分隔端口序列（1-65535），非法项静默丢弃。
function parseSeqPorts(s) {
  return String(s || "").split(/[,，\s]+/).map((x) => parseInt(x, 10))
    .filter((p) => Number.isInteger(p) && p >= 1 && p <= 65535);
}

// 敲门配置回显（knock = ConnectionView.knock 视图；null = 未配置）
function loadKnockToForm(knock) {
  knock = knock || {};
  const enabled = !!knock.enabled;
  $("f-knock-enabled").checked = enabled;
  $("knock-opts").classList.toggle("hidden", !enabled);
  $("f-knock-type").value = knock.knock_type || "sequence";
  $("f-knock-seq").value = (knock.sequence || []).join(",");
  $("f-knock-proto").value = knock.protocol || "udp";
  $("f-knock-seqtimeout").value = knock.seq_timeout || 10;
  $("f-knock-delayms").value = knock.delay_ms != null ? knock.delay_ms : 150;
  $("f-knock-spaport").value = knock.spa_port || 62201;
  $("f-knock-fwtimeout").value = knock.fw_timeout || 30;
  $("f-knock-openports").value = knock.open_ports || "tcp/3389";
  $("f-knock-key").value = "";
  $("f-knock-key").placeholder = knock && knock.has_key ? "已保存（留空保持不变）" : "SPA 加密密钥（保存于本机）";
  $("f-knock-hmac").value = "";
  $("f-knock-hmac").placeholder = knock && knock.has_hmac_key ? "已保存（留空保持不变）" : "可选，启用 HMAC-SHA256 尾签";
  syncKnockTypeVisibility();
}

// 类型切换：sequence / spa 分组可见性（SPA 留占位后续扩展 custom）
function syncKnockTypeVisibility() {
  const isSpa = $("f-knock-type").value === "spa";
  $("knock-seq-opts").classList.toggle("hidden", isSpa);
  $("knock-spa-opts").classList.toggle("hidden", !isSpa);
}

$("f-knock-enabled").addEventListener("change", (e) => {
  $("knock-opts").classList.toggle("hidden", !e.target.checked);
});
$("f-knock-type").addEventListener("change", syncKnockTypeVisibility);

// SPA 密钥随机生成：32 字节 CSPRNG → base64（本机加密存储，不上传）
$("btn-gen-key").addEventListener("click", () => {
  const buf = new Uint8Array(32);
  crypto.getRandomValues(buf);
  let bin = "";
  buf.forEach((b) => { bin += String.fromCharCode(b); });
  $("f-knock-key").value = btoa(bin);
});

// 从 knockd.conf 导入：解析 sequence / seq_timeout（注意 knockd 的
// sequence 按倒序编写——文件顺序 = 敲门的逆序，实际发送顺序取反）。
$("btn-knock-import").addEventListener("click", () => {
  const text = $("f-knock-conf").value;
  const out = $("knock-import-result");
  const lines = text.split(/\r?\n/);
  let seq = null;
  let seqTimeout = null;
  let proto = null;
  for (const line of lines) {
    const seqM = line.match(/sequence\s*=\s*(.+)/);
    if (seqM) {
      const items = seqM[1].split(",").map((x) => x.trim()).filter(Boolean);
      const parsed = items.map((it) => {
        const m = it.match(/^(\d+)(?::(udp|tcp))?$/i);
        return m ? { port: parseInt(m[1], 10), proto: (m[2] || "").toLowerCase() } : null;
      }).filter(Boolean);
      if (parsed.length) {
        seq = parsed.reverse().map((x) => x.port); // 文件倒序 → 实际发送顺序
        const protos = parsed.map((x) => x.proto).filter(Boolean);
        proto = protos.some((p) => p === "tcp") ? "tcp" : "udp";
      }
    }
    const toM = line.match(/seq_timeout\s*=\s*(\d+)/);
    if (toM) seqTimeout = parseInt(toM[1], 10);
  }
  if (!seq || !seq.length) {
    out.textContent = "解析失败：未找到 sequence 行（示例：sequence = 9000,8000,7000）";
    return;
  }
  $("f-knock-enabled").checked = true;
  $("knock-opts").classList.remove("hidden");
  $("f-knock-type").value = "sequence";
  syncKnockTypeVisibility();
  $("f-knock-seq").value = seq.join(",");
  if (proto) $("f-knock-proto").value = proto;
  if (seqTimeout) $("f-knock-seqtimeout").value = seqTimeout;
  out.textContent = `已导入 ${seq.length} 个端口：${seq.join(" → ")}${seqTimeout ? `，seq_timeout=${seqTimeout}s` : ""}`;
});

// 敲门失败分类文案（knock_invalid / knock_timeout / 命令钩子失败）
function knockErrorText(msg) {
  if (msg.includes("preConnectCommand")) return `命令钩子失败：${msg}`;
  if (msg.includes("knock_invalid")) return `敲门未通过（密钥/配置无效，请检查密钥与服务器时钟）：${msg}`;
  if (msg.includes("knock_timeout")) return `服务不可达（敲门包已发送但端口未开放）：${msg}`;
  return msg;
}

// 事件绑定
$("btn-save").addEventListener("click", saveCurrent);
$("btn-cancel").addEventListener("click", resetForm);

// ==================== F4-V2（R-20261118-016）网关高可用 UI ====================
// 备用网关列表：增删行（host/port/TLS/自签豁免），凭据与主网关共享。
function gwFallbackRow(fb) {
  fb = fb || {};
  const row = document.createElement("div");
  row.className = "gw-fb-row";
  row.style.cssText = "display:flex;gap:6px;align-items:center;margin-top:6px;flex-wrap:wrap";
  row.innerHTML = `
    <input class="gw-fb-host" type="text" placeholder="${t("form.gw.fbHostPh")}" style="flex:2;min-width:140px" value="${(fb.host || "").replace(/"/g, "&quot;")}" />
    <input class="gw-fb-port" type="text" style="width:70px" value="${fb.port || 443}" />
    <label style="display:flex;align-items:center;gap:2px"><input class="gw-fb-tls" type="checkbox" ${fb.use_tls !== false ? "checked" : ""} /><span style="white-space:nowrap">TLS</span></label>
    <label style="display:flex;align-items:center;gap:2px"><input class="gw-fb-ss" type="checkbox" ${fb.allow_self_signed ? "checked" : ""} /><span style="white-space:nowrap">${t("form.gw.fbSelfSigned")}</span></label>
    <button type="button" class="btn ghost gw-fb-del">${t("form.gw.fbRemove")}</button>`;
  row.querySelector(".gw-fb-del").addEventListener("click", () => row.remove());
  return row;
}

function renderGwFallbacks(list) {
  const box = $("gw-fallbacks");
  box.innerHTML = "";
  (list || []).forEach((fb) => box.appendChild(gwFallbackRow(fb)));
}

function collectGwFallbacks() {
  return [...document.querySelectorAll("#gw-fallbacks .gw-fb-row")].map((row) => ({
    host: row.querySelector(".gw-fb-host").value.trim(),
    port: parseInt(row.querySelector(".gw-fb-port").value, 10) || 443,
    use_tls: row.querySelector(".gw-fb-tls").checked,
    allow_self_signed: row.querySelector(".gw-fb-ss").checked,
  })).filter((fb) => fb.host);
}

$("btn-gw-fallback-add").addEventListener("click", () => {
  $("gw-fallbacks").appendChild(gwFallbackRow());
});

// 健康探测：主网关 + 全部备用网关并发探测，逐节点 绿/红 + 延迟 展示。
$("btn-gw-health").addEventListener("click", async () => {
  const out = $("gw-health-results");
  const gateways = [];
  const host = $("f-gw-host").value.trim();
  if (host) {
    gateways.push({
      host,
      port: parseInt($("f-gw-port").value, 10) || 443,
      useTls: $("f-gw-tls").value !== "off",
      allowSelfSigned: $("f-gw-self-signed").checked,
    });
  }
  for (const fb of collectGwFallbacks()) {
    gateways.push({ host: fb.host, port: fb.port, useTls: fb.use_tls, allowSelfSigned: fb.allow_self_signed });
  }
  if (!gateways.length) { out.textContent = t("form.gw.healthEmpty"); return; }
  out.textContent = t("form.gw.healthRunning");
  try {
    const results = await invoke("gateway_health_check", { gateways });
    out.innerHTML = results.map((r) => {
      const label = `${r.host}:${r.port}`;
      const dot = r.ok ? "🟢" : "🔴";
      const detail = r.ok ? `${r.latencyMs}ms` : (r.error || "failed");
      return `<div>${dot} ${label} — ${detail}</div>`;
    }).join("");
  } catch (e) {
    out.textContent = `${t("form.gw.healthFail")}: ${e}`;
  }
});
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
  // F3-V1 R-20261118-010：全屏模式才显示「目标显示器/跨所有显示器」附加区
  $("multimon-opts").classList.toggle("hidden", e.target.value !== "fullscreen");
});

// ---- F3-V1 R-20261118-010 多显示器优化 ----
// 显示器枚举（list_monitors）：「显示」下拉附加显示器选择分组；
// 枚举失败/空列表时隐藏附加区（单屏环境行为不回归）。
async function loadMonitorOptions() {
  try {
    const view = await invoke("list_monitors");
    const sel = $("f-monitor");
    // 清除动态项（保留「主显示器」缺省项）
    for (const opt of Array.from(sel.options)) {
      if (opt.dataset.dynamic) opt.remove();
    }
    if (!view || !Array.isArray(view.monitors) || view.monitors.length <= 1) {
      // 单屏（或枚举为空）：隐藏分组（默认项 = 主显示器已足够）
      return;
    }
    for (const m of view.monitors) {
      const opt = document.createElement("option");
      opt.value = String(m.id);
      opt.dataset.dynamic = "1";
      const tags = [`${m.width}x${m.height}`];
      if (m.primary) tags.push(t("form.monitor.primary"));
      opt.textContent = `${t("form.monitor.item", { n: m.id + 1 })}（${tags.join("，")}）`;
      sel.appendChild(opt);
    }
  } catch (e) {
    // 非 Windows / 命令缺失：静默隐藏（旧单屏行为）
    console.warn("list_monitors failed:", e);
  }
}
loadMonitorOptions();

// 跨所有显示器勾选 ↔ 显示器下拉互斥（span 优先）
$("f-multimon").addEventListener("change", (e) => {
  $("f-monitor").disabled = e.target.checked;
});
$("f-monitor").addEventListener("change", () => {
  if ($("f-monitor").value !== "") $("f-multimon").checked = false;
});

// 「按所选显示器分辨率填充」：把 custom 分辨率输入填为当前所选显示器尺寸
$("btn-fill-monitor-res").addEventListener("click", async () => {
  try {
    const view = await invoke("list_monitors");
    if (!view || !view.monitors?.length) return;
    // 优先用显示器下拉当前选择；缺省 = 主显示器
    const wantId = $("f-monitor").value !== "" ? parseInt($("f-monitor").value, 10) : null;
    const m = (wantId != null && view.monitors.find((x) => x.id === wantId))
      || view.monitors.find((x) => x.primary)
      || view.monitors[0];
    $("f-cw").value = String(m.width);
    $("f-ch").value = String(m.height);
  } catch (e) {
    console.warn("fill from monitor failed:", e);
  }
});
// 侧栏：新建连接（切到表单空态）、搜索、新建分组、导出/导入 JSON
$("btn-new-conn").addEventListener("click", () => { resetForm(); $("f-name").focus(); });
$("conn-search").addEventListener("input", renderList);
/* v0.3.11 网关友好输入：粘贴完整地址（https://gw:8443 / wss://…）自动拆出
   域名 / 端口 / TLS 开关；纯主机名输入不受影响 */
$("f-gw-host").addEventListener("input", () => {
  const m = $("f-gw-host").value.trim().match(/^(https?|wss?):\/\/([^/:?#]+)(?::(\d+))?/i);
  if (!m) return;
  $("f-gw-host").value = m[2];
  $("f-gw-tls").value = /^(https|wss)$/i.test(m[1]) ? "on" : "off";
  if (m[3]) $("f-gw-port").value = m[3];
});
$("btn-new-group").addEventListener("click", () => {
  // F1-V1：直接创建顶层分组（分组树上管理；子分组用节点上的「＋」按钮）
  createGroup(null);
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
$("btn-import-json").addEventListener("click", openBatchImport);

// R-20261126-008 灾备包导出：连接+分组+模板+设置 单文件备份
$("btn-dr-export").addEventListener("click", async () => {
  try {
    const d = new Date();
    const pad = (n) => String(n).padStart(2, "0");
    const path = await dialog.save({
      defaultPath: `EasyRDP-灾备包-${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const [nConn, nGroup, nTpl, hasSettings] = await invoke("export_dr_package", { path });
    flash(
      `灾备包已导出：连接 ${nConn} · 分组 ${nGroup} · 模板 ${nTpl}${hasSettings ? " · 含设置" : ""}（密码保持本机加密，跨机恢复需重填凭据）`,
      true,
    );
  } catch (e) { flash(String(e), false); }
});

// R-20261118-008 批量导入：.rdp 与 .json 混选 → 向导预览（勾选/分组/重复高亮）
// → 选去重策略 → 提交 → 结果摘要（成功/跳过/覆盖/失败 + 失败明细）。
$("btn-import").addEventListener("click", openBatchImport);

let importState = null; // { paths, entries }

async function openBatchImport() {
  try {
    let paths = await dialog.open({
      multiple: true,
      directory: false,
      filters: [
        { name: t("imp.filter.all"), extensions: ["rdp", "json"] },
        { name: "RDP", extensions: ["rdp"] },
        { name: "JSON", extensions: ["json"] },
      ],
    });
    if (!paths) return;
    if (!Array.isArray(paths)) paths = [paths];
    const preview = await invoke("import_preview", { paths });
    importState = { paths, entries: preview.entries };
    renderImportWizard(preview);
  } catch (e) { flash(String(e), false); }
}

// 目标分组下拉选项：未分组 + 分组树（父组在前、子组按层级缩进，复用 F1 groupChildren）
function impGroupOptions() {
  let html = `<option value="">${esc(t("group.ungrouped"))}</option>`;
  const walk = (pid, depth) => {
    for (const g of groupChildren(pid)) {
      const pad = "&nbsp;".repeat(depth * 4) + (depth ? "└ " : "");
      html += `<option value="${esc(g.id)}">${pad}${esc(g.name)}</option>`;
      walk(g.id, depth + 1);
    }
  };
  walk(null, 0);
  return html;
}

function renderImportWizard(preview) {
  const tbody = $("imp-tbody");
  tbody.innerHTML = "";
  const btn = $("imp-confirm");
  btn.textContent = t("imp.btn.import");
  btn.onclick = null;
  btn.disabled = false;
  const groupOpts = impGroupOptions();
  for (const e of preview.entries) {
    const tr = document.createElement("tr");
    if (e.duplicate) tr.className = "dup";
    tr.innerHTML = `
      <td><input type="checkbox" class="imp-chk" checked /></td>
      <td>${esc(e.name)}${e.duplicate ? `<span class="imp-badge">${esc(t("imp.dup"))}</span>` : ""}</td>
      <td>${esc(e.host)}</td>
      <td>${e.port}</td>
      <td>${esc(e.username)}</td>
      <td><select class="imp-group">${groupOpts}</select></td>`;
    tr.querySelector(".imp-chk").addEventListener("change", updateImpCount);
    tbody.appendChild(tr);
  }
  // 失败明细（解析失败逐条原因，不全量失败）
  const failEl = $("imp-failures");
  failEl.classList.toggle("hidden", !preview.failures.length);
  failEl.textContent = preview.failures.length
    ? t("imp.failures") + "\n" + preview.failures
        .map((f) => `${f.source}[${f.entry_index}]: ${f.reason}`)
        .join("\n")
    : "";
  $("imp-summary").classList.add("hidden");
  $("imp-summary").innerHTML = "";
  $("imp-check-all").checked = true;
  updateImpCount();
  $("import-modal").classList.remove("hidden");
}

function impStrategy() {
  return document.querySelector('input[name="imp-strategy"]:checked').value;
}

function updateImpCount() {
  const chks = [...document.querySelectorAll(".imp-chk")];
  const n = chks.filter((c) => c.checked).length;
  $("imp-count").textContent = t("imp.count", { n, total: chks.length });
  $("imp-check-all").checked = n === chks.length && n > 0;
}

function closeImportWizard() {
  $("import-modal").classList.add("hidden");
  importState = null;
}

$("imp-close").addEventListener("click", closeImportWizard);
$("import-modal").addEventListener("click", (e) => { if (e.target === $("import-modal")) closeImportWizard(); });
$("imp-check-all").addEventListener("change", (e) => {
  const on = e.target.checked;
  document.querySelectorAll(".imp-chk").forEach((c) => { c.checked = on; });
  updateImpCount();
});

$("imp-confirm").addEventListener("click", async () => {
  if (!importState) return;
  const rows = [...$("imp-tbody").querySelectorAll("tr")];
  const items = [];
  rows.forEach((tr, i) => {
    const chk = tr.querySelector(".imp-chk");
    if (!chk || !chk.checked) return;
    const e = importState.entries[i];
    items.push({
      source: e.source,
      entryIndex: e.entryIndex,
      groupId: tr.querySelector(".imp-group").value.trim(),
    });
  });
  if (!items.length) { flash(t("imp.noneSelected"), false); return; }
  const btn = $("imp-confirm");
  btn.disabled = true;
  try {
    const r = await invoke("import_commit", {
      args: { paths: importState.paths, strategy: impStrategy(), items },
    });
    const summary = $("imp-summary");
    summary.classList.remove("hidden");
    summary.innerHTML =
      `<span class="ok-c">${esc(t("imp.done", { a: r.added, s: r.skipped, o: r.overwritten, f: r.failed }))}</span>` +
      (r.failures.length
        ? `<br><span class="bad-c">${esc(r.failures.join("\n"))}</span>`
        : "");
    // 已完成导入：锁定再次提交；列表即时刷新
    await refresh();
    btn.textContent = t("imp.btnClose");
    btn.onclick = closeImportWizard;
  } catch (e) {
    flash(String(e), false);
  } finally { btn.disabled = false; }
});

// ---------- F2-V2 R-20261118-014 团队模板库 ----------
// 模板 = SessionProfile 的可共享子集（剥离凭据：网关 token/password/username
// 绝不入模板；主密码本就不在 profile 里）+ 元数据（名称/描述/标签/时间/版本）。
// 存储：templates.json（app_data_dir）；导入导出走 .erdp-template/.erdp-templates
// 包（sha256 完整性 + schema 校验在 core 层完成）。

let templates = [];        // list_templates 视图缓存
let tplImportState = null; // { path, entries }

async function refreshTemplates() {
  try {
    templates = await invoke("list_templates");
  } catch (e) {
    templates = [];
    flash(String(e), false);
  }
}

async function openTemplates() {
  await refreshTemplates();
  renderTemplates();
  $("tpl-modal").classList.remove("hidden");
}

function closeTemplates() {
  $("tpl-modal").classList.add("hidden");
}

function renderTemplates() {
  const tb = $("tpl-tbody");
  tb.innerHTML = "";
  $("tpl-empty").classList.toggle("hidden", templates.length > 0);
  for (const tp of templates) {
    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td>${esc(tp.name)}</td>
      <td class="tpl-desc" title="${esc(tp.description)}">${esc(tp.description || "—")}</td>
      <td>${esc(tp.host)}</td>
      <td>${tp.port}</td>
      <td class="tpl-desc" title="${esc((tp.tags || []).join("、"))}">${esc((tp.tags || []).join("、") || "—")}</td>
      <td>${tp.usage}</td>
      <td class="tpl-ops">
        <button class="btn sm primary" data-tact="use">${esc(t("tpl.op.use"))}</button>
        <button class="btn sm" data-tact="exp">${esc(t("tpl.op.export"))}</button>
        <button class="btn sm danger" data-tact="del">${esc(t("conn.delete"))}</button>
      </td>`;
    tr.addEventListener("click", (ev) => {
      const act = ev.target?.dataset?.tact;
      if (!act) return;
      ev.stopPropagation();
      if (act === "use") useTemplate(tp);
      if (act === "exp") exportTemplate(tp);
      if (act === "del") deleteTemplate(tp);
    });
    tb.appendChild(tr);
  }
}

// 基于模板新建连接：预填表单（凭据留空由用户填写），不自动保存
function loadTemplateToForm(tp) {
  editingId = null;
  editingMeta = null;
  showErrors(null);
  $("form-title").textContent = t("tpl.formPrefix", { n: tp.name });
  $("f-id").value = "";
  $("f-name").value = tp.name || "";
  $("f-host").value = tp.host || "";
  $("f-port").value = tp.port || 3389;
  $("f-username").value = tp.username || "";
  $("f-password").value = "";
  $("f-password").placeholder = t("form.ph.password");
  $("f-group").value = "";
  $("f-resolution").value = tp.resolutionKind || "fit_window";
  $("custom-res").classList.toggle("hidden", (tp.resolutionKind || "fit_window") !== "custom");
  $("f-cw").value = tp.customW > 0 ? String(tp.customW) : "";
  $("f-ch").value = tp.customH > 0 ? String(tp.customH) : "";
  $("multimon-opts").classList.toggle("hidden", tp.resolutionKind !== "fullscreen");
  $("f-monitor").value = tp.monitor != null ? String(tp.monitor) : "";
  $("f-multimon").checked = !!tp.multiMonitor;
  $("f-monitor").disabled = !!tp.multiMonitor;
  // 网关：非凭据字段回填，凭据留空（模板本就不含凭据）
  const gw = tp.gateway || null;
  $("gateway-section").open = !!gw;
  $("f-gw-host").value = gw ? gw.host : "";
  $("f-gw-port").value = gw ? String(gw.port || 443) : "443";
  $("f-gw-tls").value = gw && !gw.use_tls ? "off" : "on";
  $("f-gw-auth").value = gw ? gw.auth : "token";
  $("f-gw-token").value = "";
  $("f-gw-token").placeholder = t("form.ph.gwToken");
  $("f-gw-user").value = gw ? (gw.username || "") : "";
  $("f-gw-pass").value = "";
  $("f-gw-pass").placeholder = t("form.ph.gwPass");
  $("f-gw-resource").value = gw ? (gw.resource_id || "") : "";
  $("f-gw-self-signed").checked = !!(gw && gw.allow_self_signed);
  $("btn-cancel").classList.add("hidden");
}

function useTemplate(tp) {
  loadTemplateToForm(tp);
  closeTemplates();
  flash(t("tpl.loaded", { n: tp.name }));
  $("f-name").focus();
}

async function saveAsTemplate(c) {
  const desc = prompt(t("tpl.saveDescPrompt"), "") || "";
  const tags = prompt(t("tpl.saveTagsPrompt"), "") || "";
  try {
    await invoke("save_template_from_connection", {
      id: c.id,
      name: c.name || c.host,
      description: desc,
      tags,
    });
    flash(t("tpl.saved", { n: c.name || c.host }));
  } catch (e) { flash(String(e), false); }
}

async function exportTemplate(tp) {
  try {
    const path = await dialog.save({
      defaultPath: `${tp.name || tp.id}.erdp-template`,
      filters: [{ name: t("tpl.filter"), extensions: ["erdp-template", "erdp-templates"] }],
    });
    if (!path) return;
    await invoke("template_export", { ids: [tp.id], path });
    flash(t("msg.exportedOne", { n: tp.name, p: path }));
  } catch (e) { flash(String(e), false); }
}

async function deleteTemplate(tp) {
  if (!confirm(t("tpl.confirmDelete", { n: tp.name }))) return;
  try {
    await invoke("delete_template", { id: tp.id });
    await refreshTemplates();
    renderTemplates();
    flash(t("tpl.deleted", { n: tp.name }));
  } catch (e) { flash(String(e), false); }
}

// 模板包导入：选文件 → 预览（勾选/重复高亮）→ 提交入库（同 id 跳过）
$("tpl-import").addEventListener("click", async () => {
  try {
    const path = await dialog.open({
      multiple: false,
      directory: false,
      filters: [{ name: t("tpl.filter"), extensions: ["erdp-template", "erdp-templates", "json"] }],
    });
    if (!path) return;
    const preview = await invoke("template_import_preview", { path });
    tplImportState = { path, entries: preview.entries };
    renderTplImportWizard(preview);
  } catch (e) { flash(String(e), false); }
});

function renderTplImportWizard(preview) {
  const tbody = $("tpl-imp-tbody");
  tbody.innerHTML = "";
  const btn = $("tpl-imp-confirm");
  btn.textContent = t("tpl.imp.btn");
  btn.onclick = null;
  btn.disabled = false;
  for (const e of preview.entries) {
    const tr = document.createElement("tr");
    if (e.duplicate) tr.className = "dup";
    tr.innerHTML = `
      <td><input type="checkbox" class="tpl-imp-chk" checked /></td>
      <td>${esc(e.name)}${e.duplicate ? `<span class="imp-badge">${esc(t("tpl.imp.dup"))}</span>` : ""}</td>
      <td>${esc(e.description || "—")}</td>
      <td>${esc(e.host)}</td>
      <td>${e.port}</td>
      <td>${esc((e.tags || []).join("、"))}</td>`;
    tr.querySelector(".tpl-imp-chk").addEventListener("change", updateTplImpCount);
    tbody.appendChild(tr);
  }
  const failEl = $("tpl-imp-failures");
  failEl.classList.toggle("hidden", !preview.failures.length);
  failEl.textContent = preview.failures.length
    ? t("imp.failures") + "\n" + preview.failures.join("\n")
    : "";
  $("tpl-imp-summary").classList.add("hidden");
  $("tpl-imp-summary").innerHTML = "";
  $("tpl-imp-check-all").checked = true;
  updateTplImpCount();
  $("tpl-import-modal").classList.remove("hidden");
}

function updateTplImpCount() {
  const chks = [...document.querySelectorAll(".tpl-imp-chk")];
  const n = chks.filter((c) => c.checked).length;
  $("tpl-imp-count").textContent = t("imp.count", { n, total: chks.length });
  $("tpl-imp-check-all").checked = n === chks.length && n > 0;
}

function closeTplImportWizard() {
  $("tpl-import-modal").classList.add("hidden");
  tplImportState = null;
}

$("tpl-close").addEventListener("click", closeTemplates);
$("tpl-modal").addEventListener("click", (e) => { if (e.target === $("tpl-modal")) closeTemplates(); });
$("tpl-imp-close").addEventListener("click", closeTplImportWizard);
$("tpl-import-modal").addEventListener("click", (e) => { if (e.target === $("tpl-import-modal")) closeTplImportWizard(); });
$("tpl-imp-check-all").addEventListener("change", (e) => {
  const on = e.target.checked;
  document.querySelectorAll(".tpl-imp-chk").forEach((c) => { c.checked = on; });
  updateTplImpCount();
});

$("tpl-imp-confirm").addEventListener("click", async () => {
  if (!tplImportState) return;
  const rows = [...$("tpl-imp-tbody").querySelectorAll("tr")];
  const ids = [];
  rows.forEach((tr, i) => {
    const chk = tr.querySelector(".tpl-imp-chk");
    if (chk && chk.checked) ids.push(tplImportState.entries[i].id);
  });
  if (!ids.length) { flash(t("tpl.imp.noneSelected"), false); return; }
  const btn = $("tpl-imp-confirm");
  btn.disabled = true;
  try {
    const r = await invoke("template_import_commit", { path: tplImportState.path, ids });
    const summary = $("tpl-imp-summary");
    summary.classList.remove("hidden");
    summary.innerHTML =
      `<span class="ok-c">${esc(t("tpl.imp.done", { a: r[0], s: r[1] }))}</span>`;
    await refreshTemplates();
    renderTemplates();
    btn.textContent = t("imp.btnClose");
    btn.onclick = closeTplImportWizard;
  } catch (e) {
    flash(String(e), false);
  } finally { btn.disabled = false; }
});

$("btn-templates").addEventListener("click", openTemplates);

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
  // R-20261119-003：功能按钮悬停说明 tooltip + 官网手册外链
  initManualTooltips();
  // R-20261119-005：任务栏固定一次性引导（未固定 && 未「不再提示」时弹窗）
  checkTaskbarPinPrompt();
  // 语言切换即时生效：静态文案由 i18n.applyI18n 重渲染；动态列表/表单在此重绘
  window.addEventListener("easyrdp-lang-changed", () => {
    renderList();
    const cur = connections.find((x) => x.id === editingId);
    if (cur) loadToForm(cur); else resetForm();
  });
  refresh().catch((e) => flash(String(e), false));
})();

// ---------- R-20261119-005 任务栏固定一次性引导 ----------
// 根因与方案见 src-tauri/src/taskbar_pin.rs；此处仅负责提示 UI 与按钮交互。
async function checkTaskbarPinPrompt() {
  try {
    const need = await invoke("taskbar_pin_status");
    if (!need) return;
  } catch { return; } // 非检测环境（如浏览器直开 dev 页）静默跳过
  const modal = document.createElement("div");
  modal.className = "pin-modal hidden";
  modal.innerHTML = `
    <div class="pin-card">
      <div class="pin-title">${t("pin.title")}</div>
      <div class="pin-desc">${t("pin.desc")}</div>
      <div class="pin-msg" id="pin-msg"></div>
      <div class="pin-btns">
        <button class="btn primary" id="pin-btn-go">${t("pin.btn.pin")}</button>
        <button class="btn ghost" id="pin-btn-later">${t("pin.btn.later")}</button>
        <button class="btn ghost" id="pin-btn-dismiss">${t("pin.btn.dismiss")}</button>
      </div>
    </div>`;
  document.body.appendChild(modal);
  const msg = modal.querySelector("#pin-msg");
  const say = (text) => { if (msg) msg.textContent = text; };
  modal.querySelector("#pin-btn-go").addEventListener("click", async () => {
    say(t("pin.msg.working"));
    try {
      await invoke("taskbar_pin_now");
      say(t("pin.msg.done"));
      setTimeout(() => modal.remove(), 1800);
    } catch (e) {
      say(`${t("pin.msg.fail")}: ${String(e)}`);
    }
  });
  modal.querySelector("#pin-btn-later").addEventListener("click", () => modal.remove());
  modal.querySelector("#pin-btn-dismiss").addEventListener("click", async () => {
    try { await invoke("taskbar_pin_dismiss"); } catch { /* 持久化失败仅影响本次记忆 */ }
    modal.remove();
  });
  // 延迟 1.2s 弹出：等首屏渲染与窗口 show 完成后再提示，避免启动瞬间打扰
  setTimeout(() => modal.classList.remove("hidden"), 1200);
}

// EasyRDP 设置框架（R-20261116-009 设置菜单框架）。
//
// 职责：
//   1. 设置弹窗：打开/关闭、ESC 关闭、点遮罩关闭、左侧分组导航切换（每次打开回到「常规」）；
//   2. loadSettings/saveSettings：Rust 端 settings.json 持久化（settings.rs），
//      invoke 失败时 localStorage 兜底；保存即生效（内存态更新）并回显；
//   3. 默认键合并：缺字段补 DEFAULT_SETTINGS，用户新键原样保留；
//   4. R-20261116-011 主题：深/浅/跟随系统 + 强调色板，切 data-theme /
//      data-theme-accent 即时生效并同步原生 RDP 窗口标题栏（set_native_theme）；
//   5. R-20261116-012 语言：常规分组语言选择，切换即时重渲染文案（i18n.js）；
//   6. R-20261116-013 快捷方式：桌面 / 快速启动栏生成按钮，状态回显，
//      已存在时按钮显示「xxx（重新生成）」。
// 参考实现：EasySSH ui/js/settings.js 的 setPanel 分组切换逻辑。

import {
  initI18n, setLang, applyI18n, t, getLang, SUPPORTED_LANGS,
} from "./i18n.js?v=2026111602"; // 与 rdp-view.js 保持同一 URL 规格（i18n 单实例）

const LS_KEY = "easyrdp-settings-fallback"; // localStorage 兜底键（invoke 不可用时）
const LS_PENDING = "easyrdp-pending-update"; // 已发现未处理的最新版本号（R-20261118-002 常驻升级横幅）

// 默认设置键骨架（与 src-tauri/src/settings.rs 的 default_settings 保持一致；
// 后续需求逐项填充设置项时，两侧同步补键即可）
export const DEFAULT_SETTINGS = {
  autoUpdateCheck: true,
  updateFreq: "daily",
  lang: "zh-CN",
  theme: "dark",
  themeAccent: "a1",
  autoRefreshShortcuts: true,
  // F1-V2 R-20261118-013 会话快照预览（默认开；关 = 停止捕获且前端不请求）
  autoSnapshotPreview: true,
  // F6-V2（R-20261118-015）Git 同步（默认关）
  gitSyncEnabled: false,
  gitSyncRemote: "",
  gitSyncBranch: "main",
  gitSyncAuthMode: "https_token",
  gitSyncToken: "",       // 前端不回显密文；保存时空值 = 保留旧密文
  gitSyncSshKey: "",
  gitSyncAutoCommit: "each_change",
};

// 当前生效的设置（内存态；保存即更新，供后续模块直接读取）
let appSettings = { ...DEFAULT_SETTINGS };

// ---- 缺字段补默认值 + 新键保留（浅合并即可：设置值为扁平键值）----
function mergeDefaults(obj) {
  return { ...DEFAULT_SETTINGS, ...(obj && typeof obj === "object" ? obj : {}) };
}

function lsRead() {
  try { return JSON.parse(localStorage.getItem(LS_KEY)); }
  catch { return null; }
}
function lsWrite(map) {
  try { localStorage.setItem(LS_KEY, JSON.stringify(map)); } catch { /* 隐私模式等场景忽略 */ }
}

// ---- 读取：优先 Rust 端 settings.json，失败回退 localStorage，再回退默认值 ----
export async function loadSettings() {
  let stored = null;
  try {
    stored = await window.__TAURI__.core.invoke("load_settings");
  } catch {
    stored = lsRead();
  }
  appSettings = mergeDefaults(stored);
  return appSettings;
}

// ---- 保存：先写 localStorage 兜底，再持久化到 Rust 端；成功与否均更新内存态 ----
export async function saveSettings(map) {
  appSettings = mergeDefaults(map);
  lsWrite(appSettings);
  try {
    await window.__TAURI__.core.invoke("save_settings", { map: appSettings });
  } catch (e) {
    console.warn("settings.json 持久化失败（已用 localStorage 兜底）:", e);
  }
  return appSettings;
}

// 供其它模块读取当前设置（不暴露可变引用的写入口）
export function getSettings() {
  return appSettings;
}

/* ---------- 主题（R-20261116-011） ---------- */

// prefers-color-scheme 变化时若处于「跟随系统」则即时切换
const systemDark = window.matchMedia?.("(prefers-color-scheme: dark)");
systemDark?.addEventListener?.("change", () => {
  if (appSettings.theme === "system") applyTheme();
});

/** 由 theme 设置解析当前是否深色（system -> prefers-color-scheme）。 */
function resolveDark() {
  const theme = appSettings.theme || "dark";
  if (theme === "light") return false;
  if (theme === "system") return systemDark ? systemDark.matches : true;
  return true;
}

/** 应用主题与强调色：设置 html data-* 属性即时生效，并同步原生 RDP 标题栏。 */
export function applyTheme() {
  const dark = resolveDark();
  const root = document.documentElement;
  root.dataset.theme = dark ? "dark" : "light";
  root.dataset.themeAccent = appSettings.themeAccent || "a1";
  // 原生 RDP 窗口标题栏（Win32 自绘）随主题切换；失败静默（无会话时本就无窗口）
  try {
    window.__TAURI__?.core?.invoke("set_native_theme", { dark })?.catch(() => {});
  } catch { /* invoke 未注入等场景忽略 */ }
}

/** 设置项变更 -> 保存 + 应用（一个入口，保持持久化与生效同步）。 */
async function changeSetting(patch) {
  await saveSettings({ ...appSettings, ...patch });
  applyTheme();
}

/* ---------- 弹窗与分组导航（参考 EasySSH setPanel） ---------- */
const $ = (id) => document.getElementById(id);

function setPanel(name) {
  document.querySelectorAll("#settings-modal .settings-nav .item").forEach((b) => {
    const on = b.dataset.panel === name;
    b.classList.toggle("on", on);
    b.setAttribute("aria-selected", on ? "true" : "false");
  });
  document.querySelectorAll("#settings-modal .settings-panel").forEach((p) => {
    p.classList.toggle("on", p.dataset.panel === name);
  });
  const body = document.querySelector("#settings-modal .settings-body");
  if (body) body.scrollTop = 0;
}

// 打开：回显当前设置与版本号，每次打开回到「常规」分组
async function openSettings() {
  await loadSettings();
  setPanel("general");
  syncSettingsUI();
  const verEl = $("settings-about-ver");
  try {
    const v = await window.__TAURI__.app.getVersion();
    if (verEl) verEl.textContent = `EasyRDP v${v}`;
    // R-010 更新分组：当前版本回显
    const uv = $("set-update-ver");
    if (uv) uv.textContent = `v${v}`;
  } catch {
    if (verEl) verEl.textContent = "EasyRDP";
  }
  $("settings-modal").classList.remove("hidden");
  refreshShortcutButtons();
  syncGitSyncUI(); // F6-V2：Git 同步分组回显
}

function closeSettings() {
  $("settings-modal").classList.add("hidden");
}

/* ---------- 设置项回显 ---------- */
function syncSettingsUI() {
  // 语言
  const langSel = $("set-lang");
  if (langSel) langSel.value = getLang() || appSettings.lang || "zh-CN";
  // 主题单选
  const theme = appSettings.theme || "dark";
  document.querySelectorAll('input[name="set-theme"]').forEach((r) => { r.checked = r.value === theme; });
  // 强调色单选
  const accent = appSettings.themeAccent || "a1";
  document.querySelectorAll('input[name="set-accent"]').forEach((r) => { r.checked = r.value === accent; });
  // R-010 更新：自动检查开关 + 检查频率
  const autoChk = $("set-update-auto");
  if (autoChk) autoChk.checked = appSettings.autoUpdateCheck !== false;
  const freqSel = $("set-update-freq");
  if (freqSel) freqSel.value = appSettings.updateFreq || "daily";
  // F1-V2 会话快照预览开关（缺省 = 开）
  const snapChk = $("set-snapshot");
  if (snapChk) snapChk.checked = appSettings.autoSnapshotPreview !== false;
}

/* ---------- F6-V2 Git 同步（R-20261118-015） ---------- */

function syncStatus(text, cls) {
  const el = $("set-sync-status");
  if (!el) return;
  el.textContent = text;
  el.classList.toggle("ok", cls === "ok");
  el.classList.toggle("bad", cls === "bad");
}

// R-20261120-002 客户端部分：「未检测到系统 git」报错增强——
// 在状态区文本下方追加内嵌处理说明（分步指引 + 官网帮助页可点击链接，
// 复用 open_manual_url（ShellExecuteW 系统默认浏览器，Rust 侧放行 easyrdp.cn 域））。
const GIT_MISSING_HELP_URL = "https://easyrdp.cn/manual/git-sync-no-git.html";

function appendGitMissingHelp() {
  const el = $("set-sync-status");
  if (!el) return;
  // 防重复：同一状态元素只挂一次说明块
  if (el.querySelector(".git-missing-help")) return;
  const box = document.createElement("div");
  box.className = "git-missing-help";
  const lines = [
    "① 安装 Git for Windows：https://git-scm.com/download/win（默认勾选「加入 PATH」即可）；",
    "② 已安装过：确认 C:\\Program Files\\Git\\cmd 在系统 PATH 中；",
    "③ 打开新终端验证：git --version；",
    "④ 重启 EasyRDP 后提示消失；若仍提示，多为未重启或 git 在非标准路径且未加入 PATH。",
  ];
  for (const s of lines) {
    const p = document.createElement("div");
    p.className = "git-missing-help-line";
    p.textContent = s;
    box.appendChild(p);
  }
  const link = document.createElement("a");
  link.className = "git-missing-help-link";
  link.href = "#";
  link.textContent = `帮助：Git 同步「未检测到系统 git」详细手册 ↗`;
  link.addEventListener("click", (ev) => {
    ev.preventDefault();
    ev.stopPropagation();
    try {
      window.__TAURI__.core.invoke("open_manual_url", { url: GIT_MISSING_HELP_URL })
        .catch((e) => console.warn("打开帮助页失败:", e));
    } catch { /* __TAURI__ 不可用（浏览器直开 dev 页）：忽略 */ }
  });
  box.appendChild(link);
  el.appendChild(box);
}

/** 回显 Git 同步分组：设置值 + 后端实时状态（git 可用性 / 仓库 / 最近提交）。 */
async function syncGitSyncUI() {
  const enabled = $("set-sync-enabled");
  if (enabled) enabled.checked = appSettings.gitSyncEnabled === true;
  const remote = $("set-sync-remote");
  if (remote) remote.value = appSettings.gitSyncRemote || "";
  const branch = $("set-sync-branch");
  if (branch) branch.value = appSettings.gitSyncBranch || "main";
  const auth = $("set-sync-auth");
  if (auth) auth.value = appSettings.gitSyncAuthMode || "https_token";
  const token = $("set-sync-token");
  if (token) token.value = ""; // 密文不回显
  const saved = $("set-sync-token-saved");
  if (saved) saved.style.display = appSettings.gitSyncTokenSaved ? "inline" : "none";
  const ssh = $("set-sync-sshkey");
  if (ssh) ssh.value = appSettings.gitSyncSshKey || "";
  const auto = $("set-sync-auto");
  if (auto) auto.value = appSettings.gitSyncAutoCommit || "each_change";
  syncAuthModeUI();
  // 后端实时状态
  const info = $("set-sync-repo-info");
  try {
    const st = await window.__TAURI__.core.invoke("git_sync_status");
    if (info) {
      const parts = [];
      parts.push(st.git_available ? t("set.sync.git.ok") : t("set.sync.git.missing"));
      if (st.enabled) {
        parts.push(st.repo_ready ? t("set.sync.repo.ready") : t("set.sync.repo.none"));
        if (st.last_commit) parts.push(`${t("set.sync.lastCommit")} ${st.last_commit}`);
        if (st.dirty) parts.push(t("set.sync.dirty"));
      }
      info.textContent = parts.join(" · ");
    }
    if (!st.git_available) {
      syncStatus(t("set.sync.git.missing"), "bad");
      appendGitMissingHelp(); // R-20261120-002：内嵌处理说明 + 官网帮助页链接
    }
  } catch {
    if (info) info.textContent = "";
  }
}

function syncAuthModeUI() {
  const mode = $("set-sync-auth")?.value || "https_token";
  const tokenRow = $("set-sync-token-row");
  const sshRow = $("set-sync-ssh-row");
  if (tokenRow) tokenRow.style.display = mode === "https_token" ? "flex" : "none";
  if (sshRow) sshRow.style.display = mode === "ssh_key" ? "flex" : "none";
}

/** 读取 UI 表单为设置补丁；token 空值不写入（保留旧密文）。 */
function gitSyncPatchFromUI() {
  const patch = {
    gitSyncEnabled: $("set-sync-enabled")?.checked === true,
    gitSyncRemote: ($("set-sync-remote")?.value || "").trim(),
    gitSyncBranch: ($("set-sync-branch")?.value || "").trim() || "main",
    gitSyncAuthMode: $("set-sync-auth")?.value || "https_token",
    gitSyncSshKey: ($("set-sync-sshkey")?.value || "").trim(),
    gitSyncAutoCommit: $("set-sync-auto")?.value || "each_change",
  };
  const tokenVal = ($("set-sync-token")?.value || "").trim();
  if (tokenVal) patch.gitSyncToken = tokenVal; // 空值不写 → save_settings 保留旧密文
  return patch;
}

async function saveGitSyncConfig() {
  try {
    await changeSetting(gitSyncPatchFromUI());
    syncStatus(t("set.sync.saved"), "ok");
    await syncGitSyncUI();
  } catch (e) {
    syncStatus(`${t("set.update.checkFail")}：${String(e)}`, "bad");
  }
}

async function gitSyncPush() {
  const btn = $("set-sync-push");
  if (btn) btn.disabled = true;
  syncStatus(t("set.sync.pushing"));
  try {
    // 手动同步前先把 UI 中的配置保存（含远端/分支/凭据变更）
    await changeSetting(gitSyncPatchFromUI());
    const msg = await window.__TAURI__.core.invoke("git_sync_push");
    syncStatus(`${msg} ✓`, "ok");
  } catch (e) {
    syncStatus(`${t("set.sync.fail")}：${String(e)}`, "bad");
    // R-20261120-002：后端返回「未检测到系统 git」时同样附内嵌处理说明
    if (String(e).includes("未检测到系统 git")) appendGitMissingHelp();
  } finally {
    if (btn) btn.disabled = false;
    syncGitSyncUI();
  }
}

async function gitSyncPull() {
  const btn = $("set-sync-pull");
  if (btn) btn.disabled = true;
  syncStatus(t("set.sync.pulling"));
  try {
    const r = await window.__TAURI__.core.invoke("git_sync_pull");
    syncStatus(r.message, r.status === "conflict_local_wins" ? "bad" : "ok");
    // 远端恢复后刷新连接列表（main.js 监听）
    if (r.status === "remote_newer") {
      window.dispatchEvent(new CustomEvent("easyrdp-sync-pulled", { detail: { restored: r.restored } }));
    }
  } catch (e) {
    syncStatus(`${t("set.sync.fail")}：${String(e)}`, "bad");
    // R-20261120-002：后端返回「未检测到系统 git」时同样附内嵌处理说明
    if (String(e).includes("未检测到系统 git")) appendGitMissingHelp();
  } finally {
    if (btn) btn.disabled = false;
    syncGitSyncUI();
  }
}

/* ---------- 快捷方式生成（R-20261116-013） ---------- */
const SC_KINDS = [
  { kind: "desktop", btn: "sc-desktop", key: "sc.btn.desktop" },
  { kind: "quicklaunch", btn: "sc-quick", key: "sc.btn.quick" },
];
const existsMap = { desktop: false, quicklaunch: false };

function scStatus(text, cls) {
  const el = $("sc-status");
  if (!el) return;
  el.textContent = text;
  el.classList.toggle("ok", cls === "ok");
  el.classList.toggle("bad", cls === "bad");
}

function scButtonLabel(entry, exists) {
  return t(entry.key) + (exists ? t("sc.regenSuffix") : "");
}

/** 打开设置页/生成后刷新按钮文案与存在状态。 */
async function refreshShortcutButtons() {
  for (const entry of SC_KINDS) {
    const btn = $(entry.btn);
    if (!btn) continue;
    try {
      const exists = await window.__TAURI__.core.invoke("check_shortcut", { kind: entry.kind });
      existsMap[entry.kind] = !!exists;
    } catch {
      existsMap[entry.kind] = false;
    }
    btn.textContent = scButtonLabel(entry, existsMap[entry.kind]);
  }
}

async function createShortcut(entry) {
  const btn = $(entry.btn);
  btn.disabled = true;
  scStatus(`${t(entry.key)} — ${t("sc.msg.creating")}`);
  try {
    const r = await window.__TAURI__.core.invoke("create_shortcut", { kind: entry.kind });
    existsMap[entry.kind] = !!r.exists;
    if (r.ok) {
      scStatus(`${r.message} ✓`, "ok");
    } else {
      scStatus(`${t("sc.msg.fail")}：${r.message}`, "bad");
    }
  } catch (e) {
    scStatus(`${t("sc.msg.fail")}：${String(e)}`, "bad");
  } finally {
    btn.disabled = false;
    btn.textContent = scButtonLabel(entry, existsMap[entry.kind]);
  }
}

/* ---------- 在线升级检测（R-20261116-010） ---------- */

const LS_LAST_CHECK = "easyrdp-last-update-check"; // 静默检查「上次检查日」本地存储键
let updateProgressBound = false;

function upStatus(text, cls) {
  const el = $("set-update-status");
  if (!el) return;
  el.textContent = text;
  el.classList.toggle("ok", cls === "ok");
  el.classList.toggle("bad", cls === "bad");
}

/** 本地日期 YYYY-MM-DD（updateFreq=daily 的同日判重）。 */
function todayLocal() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 注册 update-progress 事件监听（仅一次）：阶段 + 进度条。 */
function bindUpdateProgress() {
  if (updateProgressBound) return;
  const listen = window.__TAURI__?.event?.listen;
  if (!listen) return;
  updateProgressBound = true;
  listen("update-progress", (e) => {
    const { phase, percent, message } = e.payload || {};
    const prog = $("set-update-progress");
    const bar = $("set-update-bar");
    if (prog) prog.style.display = "block";
    if (bar) bar.style.width = `${Math.max(0, Math.min(100, percent || 0))}%`;
    upStatus(`${t(`set.update.phase.${phase || "download"}`)}：${message || ""}（${percent ?? 0}%）`);
  }).catch(() => { updateProgressBound = false; });
}

/** 手动「检查更新」：有新版显示版本号 + 更新说明 + 下载按钮；无新版显示服务端 message。 */
async function runUpdateCheck() {
  const btn = $("set-update-check");
  const dl = $("set-update-download");
  const notes = $("set-update-notes");
  if (btn) btn.disabled = true;
  if (dl) dl.style.display = "none";
  if (notes) notes.style.display = "none";
  bindUpdateProgress();
  upStatus(t("set.update.checking"));
  try {
    const r = await window.__TAURI__.core.invoke("check_update");
    try { localStorage.setItem(LS_LAST_CHECK, todayLocal()); } catch { /* 隐私模式忽略 */ }
    if (r.has_update) {
      upStatus(t("set.update.found", { v: r.latest_version }), "ok");
      try { localStorage.setItem(LS_PENDING, r.latest_version); } catch { /* 忽略 */ }
      ensureUpdateBanner(r.latest_version);
      if (notes) {
        notes.style.display = r.notes ? "block" : "none";
        notes.textContent = r.notes || "";
      }
      if (dl) dl.style.display = "inline-block";
    } else {
      // has_update=false：已是最新则清除未处理新版记录与横幅
      try { localStorage.removeItem(LS_PENDING); } catch { /* 忽略 */ }
      document.getElementById("update-banner")?.remove();
      // has_update=false 时 message 既可能是「已是最新」也可能是错误说明，原样展示
      upStatus(r.message || t("set.update.latest"));
    }
  } catch (e) {
    upStatus(`${t("set.update.checkFail")}：${String(e)}`, "bad");
  } finally {
    if (btn) btn.disabled = false;
  }
}

/** 「下载并安装」：进度由 update-progress 事件驱动；完成后展示结果 message（含重启说明）。 */
async function downloadUpdate() {
  const dl = $("set-update-download");
  if (dl) dl.disabled = true;
  bindUpdateProgress();
  upStatus(t("set.update.downloading"));
  try {
    const r = await window.__TAURI__.core.invoke("download_and_install");
    const prog = $("set-update-progress");
    const bar = $("set-update-bar");
    if (prog && bar) { prog.style.display = "block"; bar.style.width = "100%"; }
    upStatus(r.message || t("set.update.restartNote"), "ok");
  } catch (e) {
    upStatus(`${t("set.update.checkFail")}：${String(e)}`, "bad");
    if (dl) dl.disabled = false;
  }
}

/** 静默检查发现新版本：右下角非阻断 toast（8 秒自动消失）。 */
function showUpdateToast(version) {
  const el = document.createElement("div");
  el.className = "update-toast";
  el.textContent = t("set.update.toastFound", { v: version });
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 8000);
}

/** 升级横幅（对齐 EasySSH updater.js 模式）：横幅自身承载完整升级流程——
 * 「立即升级」直接触发下载安装，下载进度/结果全部展示在横幅内（设置弹窗关着也能看到）；
 * 升级成功（检查无新版）后横幅消失；✕ 仅本轮隐藏。 */
let bannerProgressBound = false;

/** 横幅内进度：监听 update-progress 事件，把阶段/百分比直接写进横幅文本与进度条。 */
function bindBannerProgress() {
  if (bannerProgressBound) return;
  const listen = window.__TAURI__?.event?.listen;
  if (!listen) return;
  bannerProgressBound = true;
  listen("update-progress", (e) => {
    const bar = document.getElementById("update-banner");
    if (!bar) return;
    const { phase, percent, message } = e.payload || {};
    const txt = bar.querySelector(".ub-text");
    const prog = bar.querySelector(".ub-prog");
    const fill = bar.querySelector(".ub-bar");
    if (prog) prog.style.display = "block";
    if (fill) fill.style.width = `${Math.max(0, Math.min(100, percent || 0))}%`;
    if (txt) txt.textContent = `${t(`set.update.phase.${phase || "download"}`)}：${message || ""}（${percent ?? 0}%）`;
  }).catch(() => { bannerProgressBound = false; });
}

function ensureUpdateBanner(version) {
  if (document.getElementById("update-banner")) {
    document.getElementById("update-banner").remove();
  }
  const bar = document.createElement("div");
  bar.id = "update-banner";
  bar.innerHTML =
    '<span class="ub-text"></span>' +
    '<span class="ub-prog" style="display:none;flex:0 1 220px;height:4px;border-radius:2px;background:rgba(255,255,255,.25);overflow:hidden;">' +
    '<span class="ub-bar" style="display:block;height:100%;width:0%;background:#fff;transition:width .2s;"></span>' +
    "</span>" +
    '<span class="ub-actions">' +
    '<button type="button" class="ub-btn ub-primary"></button>' +
    '<button type="button" class="ub-btn ub-close">✕</button>' +
    "</span>";
  bar.querySelector(".ub-text").textContent = t("set.update.bannerFound", { v: version });
  const up = bar.querySelector(".ub-primary");
  up.textContent = t("set.update.bannerUpgrade");
  /* 立即升级：横幅内完成「检查 → 下载 → 安装 → 重启提示」全流程（不再跳设置页，
     旧实现点击了不存在的 set-update-btn 且面板选择器失配，才会跳回常规标签页）。
     v0.3.14 修复：后端 download_and_install 依赖 check_update 写入的清单缓存，
     而横幅可能来自 localStorage 的 pending 记录（如冷启动），后端缓存为空会直接报
     「请先检查更新并确认有可用升级」。此处点击时先强制跑一次 check_update，
     成功后再 download_and_install，一次点击完成升级。 */
  up.addEventListener("click", async () => {
    if (up.dataset.busy) return;
    up.dataset.busy = "1";
    up.disabled = true;
    bindBannerProgress();
    const txt = bar.querySelector(".ub-text");
    if (txt) txt.textContent = t("set.update.checking");
    try {
      /* 先确保后端清单缓存有效：check_update 失败或无升级时给出明确提示 */
      const ck = await window.__TAURI__.core.invoke("check_update");
      if (!ck?.has_update) {
        /* 服务端已无新版（或检查被拒）：清掉过期横幅与 pending 记录 */
        try { localStorage.removeItem(LS_PENDING); } catch { /* 忽略 */ }
        if (txt) txt.textContent = ck?.message || t("set.update.latest");
        setTimeout(() => bar.remove(), 3000);
        return;
      }
      try { localStorage.setItem(LS_PENDING, ck.latest_version); } catch { /* 忽略 */ }
      if (txt) txt.textContent = t("set.update.downloading");
      const r = await window.__TAURI__.core.invoke("download_and_install");
      if (txt) txt.textContent = r.message || t("set.update.restartNote");
      const fill = bar.querySelector(".ub-bar");
      const prog = bar.querySelector(".ub-prog");
      if (fill) fill.style.width = "100%";
      if (prog) prog.style.display = "block";
    } catch (e) {
      if (txt) txt.textContent = `${t("set.update.checkFail")}：${String(e)} — 可重新点击重试`;
      up.disabled = false;
      delete up.dataset.busy;
    }
  });
  bar.querySelector(".ub-close").addEventListener("click", () => bar.remove());
  const css = document.createElement("style");
  css.textContent =
    "#update-banner{position:fixed;top:40px;left:0;right:0;z-index:900;display:flex;align-items:center;justify-content:center;gap:14px;" +
    "padding:8px 16px;background:linear-gradient(90deg,#3b82f6,#2563eb);color:#fff;font-size:13px;box-shadow:0 2px 8px rgba(0,0,0,.35);}" +
    "#update-banner .ub-actions{display:flex;gap:8px;align-items:center;}" +
    "#update-banner .ub-btn{border:none;border-radius:6px;padding:4px 12px;cursor:pointer;font-size:12px;}" +
    "#update-banner .ub-btn:disabled{cursor:default;opacity:.75;}" +
    "#update-banner .ub-primary{background:#fff;color:#3b82f6;font-weight:600;}" +
    "#update-banner .ub-close{background:rgba(255,255,255,.15);color:#fff;}";
  document.head.appendChild(css);
  document.body.appendChild(bar);
}

/** 启动即展示（无需等 90s）：已有未处理新版记录时先亮横幅；已升级到该版本则清记录。 */
function showPendingBanner() {
  try {
    const pending = localStorage.getItem(LS_PENDING);
    if (pending && pending !== currentAppVersion()) ensureUpdateBanner(pending);
    else if (pending) localStorage.removeItem(LS_PENDING);
  } catch { /* 忽略 */ }
}

function currentAppVersion() {
  const el = $("app-version-tag");
  return el ? String(el.textContent || "").replace(/^v/i, "").trim() : "";
}

function scheduleSilentUpdateCheck() {
  showPendingBanner();
  const runCheck = async () => {
    try {
      const s = getSettings();
      if (!s.autoUpdateCheck) return;
      const today = todayLocal();
      // R-20261118-002 v2：取消当日配额跳过（检查请求 <1KB，成本可忽略）——
      // 每次启动都复查，确保服务端发新版当天必定被发现并挂横幅
      const r = await window.__TAURI__.core.invoke("check_update");
      try { localStorage.setItem(LS_LAST_CHECK, today); } catch { /* 忽略 */ }
      if (r?.has_update) {
        try { localStorage.setItem(LS_PENDING, r.latest_version); } catch { /* 忽略 */ }
        showPendingBanner();
        showUpdateToast(r.latest_version);
        return true;
      } else {
        try { localStorage.removeItem(LS_PENDING); } catch { /* 忽略 */ }
        document.getElementById("update-banner")?.remove();
        return true;
      }
      // 无新版 / 失败：完全静默，不打扰用户
    } catch { return false; /* 静默检查失败不打扰，由兜底复查接管 */ }
  };
  // 启动后 5s 立即检查（避免 90s 空窗期内横幅永不出现）；失败则 90s 兜底复查一次
  setTimeout(async () => {
    const ok = await runCheck();
    if (!ok) setTimeout(runCheck, 85_000);
  }, 5_000);
}

export function initSettings() {
  $("btn-settings").addEventListener("click", openSettings);
  $("settings-close").addEventListener("click", closeSettings);
  // 点击遮罩（弹窗本体以外的区域）关闭
  $("settings-modal").addEventListener("click", (e) => {
    if (e.target === $("settings-modal")) closeSettings();
  });
  // ESC 关闭
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !$("settings-modal").classList.contains("hidden")) closeSettings();
  });
  // 左侧分组导航切换（事件委托，后续新增分组按钮无需再绑）
  document.querySelector("#settings-modal .settings-nav").addEventListener("click", (e) => {
    const btn = e.target.closest(".item");
    if (btn) setPanel(btn.dataset.panel);
  });

  // ---- R-012 语言选择：切换即时生效（重渲染文案）+ 持久化 ----
  const langSel = $("set-lang");
  // 选项文案随语言列表（label 原生名，不翻译）
  if (langSel && !langSel.options.length) {
    for (const l of SUPPORTED_LANGS) {
      const opt = document.createElement("option");
      opt.value = l.code;
      opt.textContent = l.label;
      langSel.appendChild(opt);
    }
  }
  langSel?.addEventListener("change", async () => {
    const lang = langSel.value;
    await setLang(lang);            // 即时重渲染静态文案
    applyTheme();                   // 主题 label 等已随 applyI18n 刷新
    window.dispatchEvent(new CustomEvent("easyrdp-lang-changed")); // 通知 main.js 重渲染动态文案
    await changeSetting({ lang });
    syncSettingsUI();
    refreshShortcutButtons();       // 按钮文案随语言刷新
  });

  // ---- R-011 主题单选 ----
  document.querySelectorAll('input[name="set-theme"]').forEach((r) => {
    r.addEventListener("change", async () => {
      if (!r.checked) return;
      await changeSetting({ theme: r.value });
    });
  });
  // ---- R-011 强调色单选 ----
  document.querySelectorAll('input[name="set-accent"]').forEach((r) => {
    r.addEventListener("change", async () => {
      if (!r.checked) return;
      await changeSetting({ themeAccent: r.value });
    });
  });

  // ---- R-013 快捷方式生成按钮 ----
  for (const entry of SC_KINDS) {
    $(entry.btn)?.addEventListener("click", () => createShortcut(entry));
  }

  // ---- R-010 更新设置与检查/下载按钮 ----
  $("set-update-auto")?.addEventListener("change", async (e) => {
    await changeSetting({ autoUpdateCheck: e.target.checked });
  });
  $("set-update-freq")?.addEventListener("change", async (e) => {
    await changeSetting({ updateFreq: e.target.value });
  });
  // ---- F1-V2 会话快照预览开关 ----
  $("set-snapshot")?.addEventListener("change", async (e) => {
    await changeSetting({ autoSnapshotPreview: e.target.checked });
  });
  $("set-update-check")?.addEventListener("click", () => runUpdateCheck());
  $("set-update-download")?.addEventListener("click", () => downloadUpdate());

  // ---- F6-V2 Git 同步（R-20261118-015）----
  $("set-sync-save")?.addEventListener("click", () => saveGitSyncConfig());
  $("set-sync-push")?.addEventListener("click", () => gitSyncPush());
  $("set-sync-pull")?.addEventListener("click", () => gitSyncPull());
  $("set-sync-auth")?.addEventListener("change", () => syncAuthModeUI());
}

/** 启动初始化：载入设置 -> 初始化语言包 -> 应用主题 -> 安排静默更新检查。返回加载后的设置。 */
export async function bootstrapSettings() {
  await loadSettings();
  await initI18n(appSettings.lang);
  applyTheme();
  // 「跟随系统」下系统主题变化后持久化的仍是 "system"，无需回写；此处仅应用。
  // R-010：启动后 5s 静默检查更新（有新版挂常驻横幅 + 非阻断 toast）
  scheduleSilentUpdateCheck();
  return appSettings;
}

/* ---------- 组合键（F-20261117-001：SAS 菜单自定义项，全局设置） ---------- */
function getCombos() {
  const v = appSettings.customCombos;
  return Array.isArray(v) ? v.filter((c) => c && c.name && c.keys) : [];
}
async function setCombos(list) {
  await changeSetting({ customCombos: list });
  renderCombos();
}
function renderCombos() {
  const el = $("combos-list");
  if (!el) return;
  const list = getCombos();
  if (!list.length) { el.textContent = "暂无自定义组合键。"; return; }
  el.innerHTML = "";
  list.forEach((c, i) => {
    const row = document.createElement("div");
    row.className = "settings-row";
    const label = document.createElement("span");
    label.textContent = `${c.name}：${c.keys}`;
    label.style.flex = "1";
    const del = document.createElement("button");
    del.className = "btn sm danger";
    del.textContent = "删除";
    del.addEventListener("click", async () => {
      await setCombos(getCombos().filter((_, j) => j !== i));
    });
    row.appendChild(label);
    row.appendChild(del);
    el.appendChild(row);
  });
}
$("combo-add")?.addEventListener("click", async () => {
  const name = $("combo-name")?.value?.trim() || "";
  const keys = $("combo-keys")?.value?.trim() || "";
  const st = $("combos-list");
  const say = (t) => { if (st) { st.textContent = t; st.classList.add("bad"); } };
  if (!name || !keys) { say("名称与组合键都不能为空。"); return; }
  const list = getCombos();
  if (list.some((c) => c.name === name)) { say(`同名组合键已存在：${name}`); return; }
  if (list.length >= 16) { say("最多 16 条自定义组合键。"); return; }
  list.push({ name, keys });
  $("combo-name").value = "";
  $("combo-keys").value = "";
  if (st) st.classList.remove("bad");
  await setCombos(list);
});
/* ---------- 组合键输入框按键捕获（R-20261117-012） ---------- */
// focus 进入捕获模式：keydown 全部 preventDefault，捕获修饰键 + 主键，
// 格式化为后端 parse_combo_keys 认识的「Ctrl+Shift+Esc」风格串填入输入框。
// Esc 清空退出；Enter / blur 结束捕获（保留已填值）；纯修饰键按下不填入。
(function comboKeyCapture() {
  const inp = $("combo-keys");
  if (!inp) return;
  let capturing = false;
  const oldPlaceholder = inp.placeholder;

  // e.code → 组合键 token（对齐 src-tauri/src/rdp_view_final.rs::combo_token_vk）
  function codeToToken(code, key) {
    if (!code) return keyTokenFallback(key);
    const m = code.match(/^(Key|Digit)([A-Z0-9])$/);
    if (m) return m[2];
    if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
    const map = {
      Escape: "Esc", Tab: "Tab", Enter: "Enter", NumpadEnter: "Enter", Space: "Space",
      Backspace: "Backspace", Delete: "Del", Insert: "Ins", Home: "Home", End: "End",
      PageUp: "PgUp", PageDown: "PgDn", PrintScreen: "PrtSc",
      ArrowLeft: "Left", ArrowUp: "Up", ArrowRight: "Right", ArrowDown: "Down",
    };
    if (map[code]) return map[code];
    return keyTokenFallback(key);
  }
  function keyTokenFallback(key) {
    if (!key) return null;
    if (key.length === 1) {
      const c = key.toUpperCase();
      return /[A-Z0-9]/.test(c) ? c : null;
    }
    const alias = { Escape: "Esc", Delete: "Del", Insert: "Ins", PageUp: "PgUp", PageDown: "PgDn" };
    const t = alias[key] || key;
    return /^(F[1-9]|F1[0-9]|F2[0-4]|Esc|Tab|Enter|Space|Backspace|Home|End|Left|Up|Right|Down)$/.test(t) ? t : null;
  }

  function exitCapture() {
    capturing = false;
    inp.classList.remove("combo-capturing");
    inp.placeholder = oldPlaceholder;
  }

  inp.addEventListener("focus", () => {
    capturing = true;
    inp.classList.add("combo-capturing");
    inp.placeholder = "请按下组合键…（Enter 结束，Esc 退出）";
  });
  inp.addEventListener("blur", exitCapture);

  inp.addEventListener("keydown", (e) => {
    if (!capturing) return; // 非捕获模式（Enter 正常触发自身事件），不干预
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) {
      // 无修饰键的 Esc：清空并退出捕获（带修饰键的 Esc 按主键捕获）
      inp.value = "";
      exitCapture();
      inp.blur();
      return;
    }
    if (e.key === "Enter") { exitCapture(); return; }
    // 纯修饰键按下（尚无主键）：不填入，继续等待
    if (["Control", "Alt", "Shift", "Meta", "AltGraph"].includes(e.key)) return;
    const main = codeToToken(e.code, e.key);
    if (!main) return; // 未知键：忽略（见 keyTokenFallback 返回 null）
    const parts = [];
    if (e.metaKey) parts.push("Win");
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.altKey) parts.push("Alt");
    if (e.shiftKey) parts.push("Shift");
    parts.push(main);
    inp.value = parts.join("+");
    exitCapture();
  });
})();

document.querySelector("#settings-modal .settings-nav")?.addEventListener("click", (e) => {
  const btn = e.target?.closest?.("[data-panel]");
  if (btn?.dataset.panel === "combos") renderCombos();
  if (btn?.dataset.panel === "sync") syncGitSyncUI();
});

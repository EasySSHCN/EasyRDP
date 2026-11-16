// EasyRDP 远程画面视图（canvas 渲染 + 完整输入捕获）。
//
// 事件协议（与 src-tauri/src/session.rs 对照，2026-11 增量管线）：
// - 监听 `rdp-frame-<sid>`：
//     { seq, kind: "full", width, height, data: base64 JPEG, mime: "image/jpeg" }          —— 首帧/兜底
//     { seq, kind: "delta", width, height, data: base64 BGRA 增量像素, mime: "raw/bgra",
//       rects: [{x,y,w,h}] }                                                               —— 高频增量
// - 监听 `rdp-session-<sid>`：{ session_id, kind: "pointer"|"disconnected", ... }
// - 发送 `session_input(session_id, input)`：
//     { type: "MouseMove", x, y } / { type: "MouseButton", button, down, x, y }
//     / { type: "Wheel", delta } / { type: "Key", code, down }   // code = e.code
// 渲染模型：离屏 canvas（远端原始分辨率）接收全帧/增量贴图；
// 显示 canvas 由 rAF 循环在内容变化时从离屏画布缩放绘制（60Hz 平滑）。

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const viewEl = () => document.getElementById("rdp-view");
const canvasEl = () => document.getElementById("rdp-canvas");
const layoutEl = () => document.querySelector(".layout");

// 当前会话状态
let state = {
  sessionId: null,
  width: 0,
  height: 0,
  demo: false,
  unlisten: [],     // 事件退订函数
  frames: 0,        // 帧计数（FPS 统计）
  fpsTimer: null,
};

function $(id) { return document.getElementById(id); }

/** 进入画面视图：session_start + 订阅事件。conn 为连接列表条目视图。 */
export async function openSession(conn, onClosed) {
  // 帧传输（2026-11 重构）：前端 30Hz 轮询 session_pull_frame 拉取 raw
  // 二进制帧（invoke 响应走 fetch-based IPC，无 emit/Channel 大 payload
  // 在部分 webview 静默丢失的问题——真机实证后端在发而前端收不到）。
  let res;
  try {
    res = await invoke("session_start", { profileId: conn.id, password: "" });
  } catch (e) {
    throw new Error(String(e));
  }
  state.sessionId = res.session_id;
  state.width = res.width;
  state.height = res.height;
  state.demo = res.demo;
  state.lastFrameAt = Date.now();
  state.nativeView = !!res.native_view;
  offscreen = null; offDirty = false; // 新会话重置离屏画布
  if (state.nativeView) {
    // 原生渲染模式（mstsc 同款架构，非阻塞 attach）：独立标题栏窗口承载
    // 画面+输入（居中弹出、置顶抢焦点）；webview 纯连接管理器——立即切回
    // 连接中心（**消除 webview 画布误交互**：此前画布残留在前台接收了
    // 用户的鼠标键盘却无处理 = "输入无效"体感，且手指光标为 webview 链接）。
    // open 事件到达 = 窗口已建（提示已在独立窗口打开）；failed = 提示查日志。
    $("rdp-status").textContent = "正在打开独立原生窗口…";
    const unOpen = await listen(`rdp-native-view-open-${res.session_id}`, () => {
      state.sessionId = null; // 防其他路径 session_stop 误停原生会话
      viewEl().classList.add("hidden");
      layoutEl().classList.remove("hidden");
      $("btn-import").classList.remove("hidden");
      $("rdp-status").textContent = "已在独立窗口打开远程桌面（画面与输入在原生窗口内）";
      invoke("frontend_log", { sessionId: res.session_id, line: "native-view：独立原生窗口已打开，webview 回连接管理器" }).catch(() => {});
    });
    const unFail = await listen(`rdp-native-view-failed-${res.session_id}`, () => {
      $("rdp-status").textContent = "原生窗口打开失败（详见 %APPDATA%\\EasyRDP\\debug.log 的 [native-view] 行）";
      invoke("frontend_log", { sessionId: res.session_id, line: "native-view attach 失败" }).catch(() => {});
    });
    state.unlisten.push(unOpen, unFail);
    return res;
  }
  // 拉帧循环（30Hz）+ 统计（5s 汇总落 debug.log）：空响应（0xFF 占位）跳过；
  // 兼容 ArrayBuffer / TypedArray / JSON 数组三种响应形态（api 版本差异防御）。
  state.pullCount = 0; state.pullFrames = 0; state.pullBad = 0;
  // 拉帧循环（30Hz）+ 统计（5s 汇总落 debug.log）：空响应（0xFF 占位）跳过；
  // 兼容 ArrayBuffer / TypedArray / JSON 数组三种响应形态（api 版本差异防御）。
  state.pullCount = 0; state.pullFrames = 0; state.pullBad = 0;
  const pullTimer = setInterval(async () => {
    if (!state.sessionId || state.sessionId !== res.session_id) { clearInterval(pullTimer); return; }
    state.pullCount += 1;
    // 每 5 次帧拉取插 1 次光标拉取（6Hz，光标形状变化频率足够）
    if (state.pullCount % 5 === 0) {
      invoke("session_pull_cursor", { sessionId: state.sessionId })
        .then((cur) => { if (cur) applyCursor(cur); })
        .catch((e) => { if ((state.pullCurErr = (state.pullCurErr || 0) + 1) <= 3) console.warn("pull cursor 失败:", e); });
    }
    try {
      const buf = await invoke("session_pull_frame", { sessionId: state.sessionId });
      let u8 = null;
      if (buf instanceof ArrayBuffer) u8 = new Uint8Array(buf);
      else if (ArrayBuffer.isView(buf)) u8 = new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
      else if (Array.isArray(buf)) u8 = Uint8Array.from(buf);
      if (!u8 || u8.length <= 1 || u8[0] === 0xFF) return; // 无新帧占位
      state.pullFrames += 1;
      // 真实载荷分类采样（1/8，2026-11 "8 推 1 收"排查）：记录到达形态
      if (state.pullFrames % 8 === 1) {
        invoke("frontend_log", {
          sessionId: res.session_id,
          line: `pull got bytes=${u8.length} kind=${u8[0]}`,
        }).catch(() => {});
      }
      applyFrame(u8);
    } catch (e) {
      state.pullBad += 1;
      if (state.pullBad <= 3) {
        invoke("frontend_log", { sessionId: res.session_id, line: `pull 失败: ${e}` }).catch(() => {});
      }
    }
  }, 33);
  state.unlisten.push(() => clearInterval(pullTimer));
  // pull 统计心跳（5s 落 debug.log）
  const pullStats = setInterval(() => {
    if (!state.sessionId) { clearInterval(pullStats); return; }
    invoke("frontend_log", {
      sessionId: res.session_id,
      line: `pull 统计: 拉取=${state.pullCount} 帧=${state.pullFrames} 解析失败=${state.pullBad} frames累计=${state.framesTotal}`,
    }).catch(() => {});
    state.pullCount = 0; state.pullFrames = 0; state.pullBad = 0;
  }, 5000);
  state.unlisten.push(() => clearInterval(pullStats));
  // 窗口自适应：custom 分辨率 → 窗口调整为协商尺寸（1:1 显示）；
  // fit_window/fullscreen 保持现有窗口（智能缩放）。demo 不调整。
  if (res.resolution_kind === "custom" && !res.demo) {
    state.windowAdjusted = await fitWindowToSession(res.width, res.height);
  }

  // 切换视图：隐藏连接中心，显示画面
  layoutEl().classList.add("hidden");
  $("btn-import").classList.add("hidden");
  viewEl().classList.remove("hidden");
  $("rdp-title").textContent = `${conn.name || conn.host} — ${conn.host}:${conn.port}`;
  const canvas = canvasEl();
  canvas.width = res.width;
  canvas.height = res.height;
  canvas.style.aspectRatio = `${res.width} / ${res.height}`;
  canvas.focus();

  // 远端光标：改走 session_pull_cursor 轮询（与帧同路的可靠 pull 通道；
  // emit 事件通道已被真机证实不可靠）。轮询循环内每 5 次帧拉取插 1 次
  // 光标拉取（6Hz 足够）。null/empty = 暂无更新（保持现状）。
  let cursorEvents = 0;
  const applyCursor = (p) => {
    cursorEvents += 1;
    if (cursorEvents % 10 === 1) {
      invoke("frontend_log", {
        sessionId: res.session_id,
        line: `cursor pull #${cursorEvents} ${p.width}x${p.height} reset=${p.reset}`,
      }).catch(() => {});
    }
    const canvas = canvasEl();
    if (p.reset || !p.width || !p.height) {
      canvas.style.cursor = "default"; // PointerSystem：系统默认（箭头）
      return;
    }
    try {
      const bytes = b64ToBytes(p.rgba_base64);
      const c = document.createElement("canvas");
      c.width = p.width;
      c.height = p.height;
      const ctx = c.getContext("2d");
      const imgData = ctx.createImageData(p.width, p.height);
      imgData.data.set(bytes.subarray(0, p.width * p.height * 4));
      ctx.putImageData(imgData, 0, 0);
      canvas.style.cursor = `url(${c.toDataURL("image/png")}) ${p.hotspot_x} ${p.hotspot_y}, auto`;
    } catch (e) {
      console.warn("cursor update failed:", e);
    }
  };
  const unSess = await listen(`rdp-session-${res.session_id}`, (ev) => {
    if (ev.payload.kind === "clipboard") {
      // 远端 -> 本地：引擎已写入系统剪贴板，这里仅做状态提示
      const n = (ev.payload.text || "").length;
      $("rdp-status").textContent = `剪贴板已从远端同步 ${n} 字符`;
    } else if (ev.payload.kind === "disconnected") {
      // 会话结束：回连接中心并提示
      closeSession();
      onClosed?.(`会话已结束：${ev.payload.reason || "未知原因"}`, false);
    }
  });
  // 保留 openSession 中可能已推入的回退帧监听（base64 路径）
  state.unlisten = [...state.unlisten, unSess];
  startCompositor();

  // FPS 统计（口径：帧批次/秒 + 更新像素面积 MP/s）
  state.updatedPixels = 0;
  state.fpsTimer = setInterval(() => {
    const fps = state.frames;
    const mp = state.updatedPixels / 1e6;
    state.frames = 0;
    state.updatedPixels = 0;
    $("rdp-status").textContent =
      `${res.width}×${res.height} · ${fps} 帧/秒 · 更新 ${mp.toFixed(1)} MP/s` +
      (res.demo ? " · 演示模式（stub）" : "");
  }, 1000);
}

/** 断开并回到连接中心（幂等）。 */
export function closeSession() {
  if (state.fpsTimer) { clearInterval(state.fpsTimer); state.fpsTimer = null; }
  stopCompositor();
  // 恢复默认窗口尺寸（custom 会话曾自适应放大；2026-11 用户需求）
  if (state.windowAdjusted) { restoreWindowSize(); state.windowAdjusted = false; }
  for (const un of state.unlisten) { try { un(); } catch { /* 已退订 */ } }
  state.unlisten = [];
  const sid = state.sessionId;
  state.sessionId = null;
  if (sid) invoke("session_stop", { sessionId: sid }).catch(() => {});
  viewEl().classList.add("hidden");
  layoutEl().classList.remove("hidden");
  $("btn-import").classList.remove("hidden");
  return sid != null;
}

/** 画布尺寸 -> 远程逻辑坐标（canvas 有 aspect-ratio 锁定，直接按 rect 比例放大）。 */
export function toRemote(clientX, clientY) {
  const canvas = canvasEl();
  const rect = canvas.getBoundingClientRect();
  const x = Math.floor(((clientX - rect.left) / rect.width) * state.width);
  const y = Math.floor(((clientY - rect.top) / rect.height) * state.height);
  const clamp = (v, max) => Math.max(0, Math.min(max - 1, v));
  return [clamp(x, state.width), clamp(y, state.height)];
}

export async function sendInput(payload) {
  if (!state.sessionId) return;
  try {
    await invoke("session_input", { sessionId: state.sessionId, input: payload });
  } catch (e) {
    // 会话可能刚结束；其余失败（如引擎未 ACTIVE）打印告警供排查
    console.warn("session_input 失败:", payload, e);
  }
}

/** 本地 -> 远端剪贴板：读本地剪贴板文本并推送（失败静默，如权限拒绝/非文本）。 */
async function syncClipboardToRemote() {
  if (!state.sessionId) return;
  try {
    const text = await navigator.clipboard.readText();
    if (text) {
      await invoke("session_clipboard_set", { sessionId: state.sessionId, text });
    }
  } catch (e) {
    // 权限被拒 / 剪贴板为非文本 / 会话已结束：忽略（α 仅支持文本）
    console.warn("clipboard sync skipped:", e);
  }
}

// ---- 帧绘制（增量管线） ----
// 离屏画布：远端原始分辨率，接收全帧 JPEG / 增量矩形贴图；
// 显示画布由 rAF 循环在离屏内容变化后从离屏画布缩放绘制。
//
// 2026-11 残影修复：**所有绘制操作串行化**（promise 链）。此前全帧 JPEG
// 走 img 异步解码，增量走同步贴图——拖拽等连续变化中，5s 兜底全帧（旧快照）
// 的 onload 会晚于其后增量到达而**覆盖新内容** → 残影色块。串行化后严格
// 按事件到达顺序应用，全帧解码期间到达的增量排队在其后。
let offscreen = null;          // 离屏 canvas
let offDirty = false;          // 离屏内容变化标记（rAF 消费）
let rafHandle = null;          // rAF 循环句柄
let drawChain = Promise.resolve(); // 绘制操作串行链

function ensureOffscreen(w, h) {
  if (!offscreen) {
    offscreen = document.createElement("canvas");
    offscreen.width = w;
    offscreen.height = h;
  } else if (offscreen.width !== w || offscreen.height !== h) {
    offscreen.width = w; // 重置尺寸会清空内容：仅在全帧（带完整画面）时调用
    offscreen.height = h;
  }
  return offscreen;
}

/** base64 -> Uint8Array */
function b64ToBytes(b64) {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

// ---- 二进制帧协议解析（与 src-tauri build_frame_message 对照，全 LE）----
// u8 kind(0=full,1=delta) / u32 w / u32 h / u32 seq / u16 nRects /
// u32 payloadLen / nRects × (u32 x,y,w,h) / payloadLen 字节 payload
// （pull 响应可含多条连续帧，按 payloadLen 迭代解析）
function parseBinaryFrames(u8) {
  const frames = [];
  // 单 DataView 全缓冲 + 绝对偏移（此前按 off 切片视图 + 绝对偏移双重
  // 基准 → off>0 时越界 RangeError → 整条消息剩余帧全部丢弃 = 残影/黑块）
  const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  let off = 0;
  while (off + 19 <= u8.length) {
    const kind = u8[off];
    const width = dv.getUint32(off + 1, true);
    const height = dv.getUint32(off + 5, true);
    const seq = dv.getUint32(off + 9, true);
    const nRects = dv.getUint16(off + 13, true);
    const payloadLen = dv.getUint32(off + 15, true);
    let p = off + 19;
    const rects = [];
    for (let i = 0; i < nRects; i++) {
      rects.push({
        x: dv.getUint32(p, true),
        y: dv.getUint32(p + 4, true),
        w: dv.getUint32(p + 8, true),
        h: dv.getUint32(p + 12, true),
      });
      p += 16;
    }
    const payload = u8.subarray(p, p + payloadLen);
    frames.push({ kind, width, height, seq, rects, payload });
    off = p + payloadLen;
  }
  return frames;
}

/** 窗口尺寸辅助（兼容不同 api 内嵌版本的模块挂载位置）。
 * 返回 { getCurrentWindow, LogicalSize } 或 null（api 不可用时跳过窗口操作）。 */
function tauriWindowApi() {
  const T = window.__TAURI__;
  const getCurrentWindow = T?.window?.getCurrentWindow ?? T?.core?.getCurrentWindow;
  const LogicalSize = T?.dpi?.LogicalSize ?? T?.window?.LogicalSize ?? T?.core?.LogicalSize;
  if (!getCurrentWindow || !LogicalSize) return null;
  return { getCurrentWindow, LogicalSize };
}

/** 窗口自适应会话分辨率（2026-11 用户需求 + 纵向偏移修复）：
 * custom 分辨率 → 主窗口客户区调整为协商尺寸（画面 1:1 无缩放，
 * 超出屏幕工作区时钳制）；fit_window 保持现有窗口（智能缩放）。
 * 偏移修复（2026-11）：此前固定 +40px 与实际 chrome（自绘标题栏 36 +
 * 工具条 + stage padding + 不可见边框）不符 → 画布纵向压缩/letterbox。
 * 现用 innerSize 反馈校准：setSize 后读实际客户区，按差值二次补偿，
 * 保证客户区精确等于「画面 + 实测 chrome」。 */
async function fitWindowToSession(resWidth, resHeight) {
  const api = tauriWindowApi();
  if (!api) return false;
  try {
    const win = api.getCurrentWindow();
    // 实测 chrome 高度（自绘标题栏 + rdp 工具条 + stage 上下 padding）
    let chromeH = 0;
    try {
      const tb = document.querySelector(".titlebar");
      const bar = document.querySelector(".rdp-toolbar");
      const stage = document.querySelector(".rdp-stage");
      const padY = stage ? (parseFloat(getComputedStyle(stage).paddingTop) + parseFloat(getComputedStyle(stage).paddingBottom)) : 0;
      chromeH = (tb?.offsetHeight || 0) + (bar?.offsetHeight || 0) + padY;
    } catch { chromeH = 40; }
    const targetInnerH = resHeight + Math.round(chromeH);
    const targetW = resWidth;
    await win.setSize(new api.LogicalSize(targetW, targetInnerH));
    // 反馈校准：setSize 设置的是外框尺寸，边框/装饰差值用 innerSize 回差补偿。
    // 时序修复（2026-11）：setSize 后 innerSize 可能短暂返回旧值——轮询至
    // 稳定（两次一致）或超时，再校准一次。
    try {
      const scale = (await win.scaleFactor?.()) ?? 1;
      const readInner = async () => {
        const inner = await win.innerSize?.();
        return inner ? inner.height / scale : null;
      };
      let prev = await readInner();
      for (let i = 0; i < 10 && prev !== null; i++) {
        await new Promise((r) => setTimeout(r, 60));
        const cur = await readInner();
        if (cur === prev) break; // 已稳定
        prev = cur;
      }
      if (prev !== null) {
        const delta = targetInnerH - prev;
        if (Math.abs(delta) > 1) {
          await win.setSize(new api.LogicalSize(targetW, targetInnerH + delta));
        }
      }
    } catch { /* innerSize api 不可用：跳过校准 */ }
    return true;
  } catch (e) {
    console.warn("窗口自适应失败:", e);
    return false;
  }
}

/** 断开时恢复默认窗口尺寸（tauri.conf 1080×700）。 */
function restoreWindowSize() {
  const api = tauriWindowApi();
  if (!api) return;
  api.getCurrentWindow().setSize(new api.LogicalSize(1080, 700)).catch?.(() => {});
}
function decodeImage(dataUrl) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = dataUrl;
  });
}

/** 实际应用一帧（全帧/增量），在串行链内执行。
 * full：bytes = JPEG 字节 → Blob URL 解码（无 base64 字符串）；
 * delta：bytes = BGRA 打包 → 换 RGBA 贴矩形。 */
/** 实际应用一帧（全帧/增量），在串行链内执行。
 * full：payload = JPEG 字节 → Blob URL 解码（无 base64 字符串）；
 * delta：payload = BGRA 打包 → 换 RGBA 贴矩形。 */
async function doApplyFrame(frame) {
  const { kind, width: w, height: h, rects, payload } = frame;
  if (!w || !h) return;
  const oc = ensureOffscreen(w, h);
  const octx = oc.getContext("2d");

  if (kind === 0) {
    try {
      const url = URL.createObjectURL(new Blob([payload], { type: "image/jpeg" }));
      const img = await decodeImage(url);
      octx.drawImage(img, 0, 0, w, h);
      offDirty = true;
      URL.revokeObjectURL(url);
    } catch { /* 解码失败：跳过该帧（增量继续打补丁） */ }
    return;
  }
  if (kind === 1) {
    let off = 0;
    for (const r of rects) {
      const len = r.w * r.h * 4;
      if (off + len > payload.length) break; // 防御：数据与矩形不符时截断
      const imgData = octx.createImageData(r.w, r.h);
      const d = imgData.data;
      // 引擎交付 BGRA32（GDI 合成缓冲），ImageData 需要 RGBA：交换 R/B；
      // alpha 置不透明（GDI 合成缓冲的 A 通道无语义）。
      for (let i = 0; i < len; i += 4) {
        d[i] = payload[off + i + 2];
        d[i + 1] = payload[off + i + 1];
        d[i + 2] = payload[off + i];
        d[i + 3] = payload[off + i + 3] || 255;
      }
      octx.putImageData(imgData, r.x, r.y);
      off += len;
    }
    offDirty = true;
  }
}

/** 二进制 raw 消息（可含多条帧）→ 迭代解析 → 串行绘制链。
 * 解析异常不中断：逐帧 try，坏帧跳过（计数落 debug.log）。 */
function applyFrame(msg) {
  const u8 = msg instanceof Uint8Array
    ? msg
    : msg instanceof ArrayBuffer ? new Uint8Array(msg) : null;
  if (!u8 || u8.length < 1 || u8[0] === 0xFF) return; // 空占位
  let frames;
  try {
    frames = parseBinaryFrames(u8);
  } catch (e) {
    if ((state.badMsgCount = (state.badMsgCount || 0) + 1) <= 5) {
      invoke("frontend_log", {
        sessionId: state.sessionId,
        line: `parseBinaryFrames 异常: ${e}`,
      }).catch(() => {});
    }
    return;
  }
  for (const frame of frames) applyFrameObject(frame);
}

/** base64 事件回退路径：rdp-frame 载荷 → 帧对象 → 串行绘制链。 */
/** base64 事件载荷转换（保留：诊断对照用，主管线为 pull 轮询）。 */
function applyFrameEvent(p) {
  applyFrameObject({
    kind: p.kind === "full" ? 0 : 1,
    width: p.width,
    height: p.height,
    rects: p.rects || [],
    bytes: b64ToBytes(p.data),
  });
}

/** 应用一帧（帧对象）：统计口径（帧批次 + 更新像素面积）+ 串行链。 */
function applyFrameObject(frame) {
  if (!frame || !frame.width || !frame.height) return;
  state.frames += 1;
  state.framesTotal = (state.framesTotal || 0) + 1;
  state.lastFrameAt = Date.now();
  if (frame.kind === 1) {
    for (const r of frame.rects) state.updatedPixels += r.w * r.h;
  } else {
    state.updatedPixels += frame.width * frame.height;
  }
  // 串行链：按到达顺序应用，杜绝旧全帧覆盖新增量（残影根因）
  drawChain = drawChain.then(() => doApplyFrame(frame)).catch(() => {});
}

/** rAF 合成循环：离屏内容变化时把远端画面缩放绘制到显示画布（60Hz）。 */
function startCompositor() {
  if (rafHandle) return;
  const tick = () => {
    rafHandle = requestAnimationFrame(tick);
    if (!state.sessionId) return;
    if (!offDirty) return;
    offDirty = false;
    const canvas = canvasEl();
    const ctx = canvas.getContext("2d");
    // 服务端协商分辨率变化时同步显示画布（aspect-ratio/坐标基准一并跟随）
    if (offscreen && (canvas.width !== offscreen.width || canvas.height !== offscreen.height)) {
      canvas.width = offscreen.width;
      canvas.height = offscreen.height;
      canvas.style.aspectRatio = `${offscreen.width} / ${offscreen.height}`;
      state.width = offscreen.width;
      state.height = offscreen.height;
    }
    if (offscreen) ctx.drawImage(offscreen, 0, 0, canvas.width, canvas.height);
  };
  rafHandle = requestAnimationFrame(tick);
}

function stopCompositor() {
  if (rafHandle) { cancelAnimationFrame(rafHandle); rafHandle = null; }
  offscreen = null;
  offDirty = false;
}

// ---- 输入绑定（模块加载即绑定；仅会话视图可见时生效）----
export function bindInput() {
  const canvas = canvasEl();

  canvas.addEventListener("mousemove", (e) => {
    if (!state.sessionId) return;
    // 2026-11 鼠标迟钝优化：浏览器 mousemove 高达 60-144Hz，每次 invoke
    // 都有 IPC 往返成本。节流到 ~30Hz（33ms），只发最新位置（指针事件
    // 丢中间点无感知损失）。
    const now = performance.now();
    if (now - (state.lastMoveSent || 0) < 33) {
      state.pendingMove = [e.clientX, e.clientY];
      if (!state.moveTimer) {
        state.moveTimer = setTimeout(() => {
          state.moveTimer = null;
          const pm = state.pendingMove;
          state.pendingMove = null;
          if (pm && state.sessionId) {
            const [x, y] = toRemote(pm[0], pm[1]);
            sendInput({ type: "MouseMove", x, y });
            state.lastMoveSent = performance.now();
          }
        }, 33);
      }
      return;
    }
    state.lastMoveSent = now;
    const [x, y] = toRemote(e.clientX, e.clientY);
    sendInput({ type: "MouseMove", x, y });
  });
  canvas.addEventListener("mousedown", (e) => {
    if (!state.sessionId) return;
    canvas.focus();
    const [x, y] = toRemote(e.clientX, e.clientY);
    const button = e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
    sendInput({ type: "MouseButton", button, down: true, x, y });
    e.preventDefault();
  });
  canvas.addEventListener("mouseup", (e) => {
    if (!state.sessionId) return;
    const [x, y] = toRemote(e.clientX, e.clientY);
    const button = e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
    sendInput({ type: "MouseButton", button, down: false, x, y });
    e.preventDefault();
  });
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  canvas.addEventListener("wheel", (e) => {
    if (!state.sessionId) return;
    // 横向优先：|deltaX| 明显大于 |deltaY| 时按横向滚轮处理
    //（Shift+滚轮浏览器会转成 deltaX；触摸板横扫原生 deltaX）。
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      // deltaX 正值向右：HWHEEL rotation 正值向右
      sendInput({ type: "WheelHorizontal", delta: Math.round(e.deltaX) });
      e.preventDefault();
      return;
    }
    // deltaY 正值向下滚；协议 delta 正值向上 => 取反，并按 Windows WHEEL_DELTA 归一
    const delta = -Math.round(e.deltaY);
    sendInput({ type: "Wheel", delta });
    e.preventDefault();
  }, { passive: false });

  // 键盘：画布聚焦时捕获，传 e.code（VK 映射在后端做）
  canvas.addEventListener("keydown", (e) => {
    if (!state.sessionId) return;
    // Win 键发送后的 200ms 输入锁：防用户按键与程序化 Win 键交错
    if (Date.now() < (state.inputLockUntil || 0)) { e.preventDefault(); return; }
    // 本地 -> 远端剪贴板钩子：Ctrl+C/X/V 在画布捕获时先读取本地剪贴板
    // 推送到远端（CLIPRDR 公告；α 方案 A/B 由后端决策），再照常转发按键。
    // 远端 -> 本地无需前端参与：引擎收到远端 FormatList 后写系统剪贴板。
    if (e.ctrlKey && ["KeyC", "KeyX", "KeyV"].includes(e.code)) {
      syncClipboardToRemote();
    }
    sendInput({ type: "Key", code: e.code, down: true });
    // LED 同步：NumLock/CapsLock/ScrollLock 按下后，把本地 LED toggle 状态
    // 推到远端（服务端 LED 与客户端一致后 Numpad 数字才直接生效）。
    if (["NumLock", "CapsLock", "ScrollLock"].includes(e.code)) {
      invoke("sync_keyboard_leds", { sessionId: state.sessionId }).catch((err) =>
        console.warn("sync_keyboard_leds 失败:", err)
      );
    }
    e.preventDefault();
  });
  canvas.addEventListener("keyup", (e) => {
    if (!state.sessionId) return;
    if (Date.now() < (state.inputLockUntil || 0)) { e.preventDefault(); return; }
    sendInput({ type: "Key", code: e.code, down: false });
    // LED 同步补发：NumLock 的 toggle 在 keyup 后才翻转，keydown 时读到的
    // GetKeyState 是旧状态（2026-11 实测：首次按下同步到旧值）。
    if (["NumLock", "CapsLock", "ScrollLock"].includes(e.code)) {
      invoke("sync_keyboard_leds", { sessionId: state.sessionId }).catch(() => {});
    }
    e.preventDefault();
  });

  // 工具条
  $("rdp-disconnect").addEventListener("click", () => closeSession());
  // ⊞ Win：本地系统截获物理 Win 键（keydown 到不了 webview），按钮走后端
  // command 程序化发送 MetaLeft down/up；期间 200ms 输入锁防用户按键干扰。
  $("rdp-win").addEventListener("click", async () => {
    if (!state.sessionId) return;
    state.inputLockUntil = Date.now() + 200;
    invoke("frontend_log", { sessionId: state.sessionId, line: "Win 按钮点击" }).catch(() => {});
    try {
      await invoke("send_win_key", { sessionId: state.sessionId });
      $("rdp-status").textContent = "已发送 Win 键";
      invoke("frontend_log", { sessionId: state.sessionId, line: "send_win_key 成功" }).catch(() => {});
    } catch (e) {
      invoke("frontend_log", { sessionId: state.sessionId, line: `send_win_key 失败: ${e}` }).catch(() => {});
      console.warn("send_win_key 失败:", e);
    }
  });
  $("rdp-fullscreen").addEventListener("click", () => {
    const stage = document.querySelector(".rdp-stage");
    if (document.fullscreenElement) document.exitFullscreen();
    else stage.requestFullscreen?.();
  });
}

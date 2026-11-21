// EasyRDP 多语言（R-20261116-012）。
//
// 语言包外置：i18n-dict-<lang>.js 四个文件（zh-CN/zh-TW/en/ko），
// 本模块用动态 import 按需加载并缓存；缺 key 回退 zh-CN，再缺回退 key 本身。
//
// 用法：
//   静态文案：HTML 元素加 data-i18n="key"（textContent）/ data-i18n-ph="key"
//   （placeholder）/ data-i18n-title="key"（title）/ data-i18n-html="key"（innerHTML），
//   切换语言后调用 applyI18n() 全量重渲染；
//   动态文案：JS 里 t("key", { h: host, p: port })，{x} 占位符替换。
//
// 注意：本模块必须只被一个 URL 规格导入（./i18n.js?v=2026111602），
// 否则 ES 模块按 URL 缓存会产生多实例、状态分裂。

export const SUPPORTED_LANGS = [
  { code: "zh-CN", label: "简体中文" },
  { code: "zh-TW", label: "繁體中文" },
  { code: "en", label: "English" },
  { code: "ko", label: "한국어" },
];

const FALLBACK_LANG = "zh-CN";
const V = "v=2026111603"; // 与 index.html 缓存参数同步递增

const cache = new Map(); // lang -> dict
let currentLang = FALLBACK_LANG;

async function loadDict(lang) {
  if (cache.has(lang)) return cache.get(lang);
  try {
    const mod = await import(`./i18n-dict-${lang}.js?${V}`);
    cache.set(lang, mod.default || {});
  } catch (e) {
    console.warn(`语言包加载失败: ${lang}`, e);
    cache.set(lang, {});
  }
  return cache.get(lang);
}

/** 初始化：加载回退包 + 指定语言包，并全量渲染静态文案。 */
export async function initI18n(lang) {
  await loadDict(FALLBACK_LANG);
  await setLang(lang || FALLBACK_LANG);
}

/** 切换语言（即时生效：重渲染全部 data-i18n 文案）。 */
export async function setLang(lang) {
  currentLang = SUPPORTED_LANGS.some((l) => l.code === lang) ? lang : FALLBACK_LANG;
  await loadDict(currentLang);
  document.documentElement.lang = currentLang;
  applyI18n();
}

export function getLang() {
  return currentLang;
}

/**
 * 取译文：当前语言 -> zh-CN 回退 -> key 本身。
 * params：{x} 占位符替换表，如 t("msg.connecting", { h: "1.2.3.4", p: 3389 })。
 */
export function t(key, params) {
  const dict = cache.get(currentLang) || {};
  const fb = cache.get(FALLBACK_LANG) || {};
  let s = dict[key] ?? fb[key] ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) s = s.replaceAll(`{${k}}`, String(v));
  }
  return s;
}

/** 全量重渲染静态文案（切换语言后调用；initI18n/setLang 内部已调用）。 */
export function applyI18n(root = document) {
  root.querySelectorAll("[data-i18n]").forEach((el) => { el.textContent = t(el.dataset.i18n); });
  root.querySelectorAll("[data-i18n-ph]").forEach((el) => { el.placeholder = t(el.dataset.i18nPh); });
  root.querySelectorAll("[data-i18n-title]").forEach((el) => { el.title = t(el.dataset.i18nTitle); });
  root.querySelectorAll("[data-i18n-html]").forEach((el) => { el.innerHTML = t(el.dataset.i18nHtml); });
  root.querySelectorAll("[data-i18n-value]").forEach((el) => { el.value = t(el.dataset.i18nValue); });
}

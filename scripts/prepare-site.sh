#!/usr/bin/env bash
# ============================================================================
# prepare-site.sh —— 官网随版内容自动化准备（RELEASE 第 8 步前置，AUTO-SITE 20261128）
# ----------------------------------------------------------------------------
# 作用：把发布流程中第 8 步前的人工官网动作全部自动化：
#   ① changelog.html 自动生成 v$VER 条目（README 「### v$VER」小节为权威源）
#      + 「当前构建」徽章从旧版本块迁移到新块（改前备份）
#   ② download.html「当前版本」描述文案自动改写（README 小节首条摘要；
#      版本串仍由 release.sh 第 8 步全局替换）
#   ③ docs/官网更新历史记录.md 自动追加本次事件 + node gen-site-updates.js
#      重新生成 site-updates.json（改前备份）
#   ④ manual/ 随版维护检测：tooltip.js MANUAL_LINKS 与 manual/ 目录比对
#      + README 小节提到的按钮/入口名称全 manual grep——缺页自动用既有页生成
#      占位页并注入 toc/index 条目；名称 grep 不到输出「manual 待更新清单」
#      并以非零码退出（发布会话按清单补文案后重跑本脚本即可）
#
# 用法：
#   bash scripts/prepare-site.sh <版本号>          # 正常发布（release.sh 自动调用）
#   bash scripts/prepare-site.sh 0.7.0             # 单独执行（8a 之前任意时点）
#
# 测试隔离环境变量（不污染真实站点源）：
#   SITE=<目录>        站点根（默认 EasyRDP/easyrdp-site）
#   UPDATES_MD=<文件>  官网更新历史记录 md（默认 docs/官网更新历史记录.md）
#   MANUAL_SRC=<文件>  tooltip.js 路径（默认 easyrdp-client/src/ui/tooltip.js）
#   SKIP_NODE=1        跳过 gen-site-updates.js（测试无 node 时）
#
# 注意（grep -q 管道陷阱）：本脚本校验逻辑一律 grep 于变量/文件，不在校验管道中用 -q。
# 退出码：0 成功；2 manual 待更新清单非空（需发布会话补文案后重跑）；1 其他失败。
# ============================================================================
set -uo pipefail

VER="${1:-}"
if [ -z "$VER" ] || ! printf '%s' "$VER" | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' >/dev/null; then
  echo "用法: bash scripts/prepare-site.sh <版本号>  (如 0.7.0)"; exit 1
fi
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SITE="${SITE:-$ROOT/easyrdp-site}"
UPDATES_MD="${UPDATES_MD:-$ROOT/docs/官网更新历史记录.md}"
MANUAL_SRC="${MANUAL_SRC:-$ROOT/easyrdp-client/src/ui/tooltip.js}"
README="$ROOT/README.md"

for f in "$README" "$SITE/changelog.html" "$SITE/download.html" "$UPDATES_MD"; do
  [ -f "$f" ] || { echo "✗ 缺少输入文件: $f"; exit 1; }
done

TAG="v$VER"
# ---- 改前备份（时间戳目录）----
TS=$(date +%Y%m%d-%H%M%S)
BK="$SITE/.backup/$TS"
mkdir -p "$BK"
cp "$SITE/changelog.html" "$BK/" && cp "$SITE/download.html" "$BK/" && cp "$UPDATES_MD" "$BK/官网更新历史记录.md" \
  && echo "==> 备份完成: ${BK#$ROOT/}" || { echo "✗ 备份失败，终止"; exit 1; }
# manual 新增页改动也先整目录备份
if [ -d "$SITE/manual" ]; then cp -r "$SITE/manual" "$BK/manual"; fi

# ---- 单次 python 手术：README 解析 + changelog/download/更新记录/manual ----
export PSITE_VER="$VER" PSITE_SITE="$SITE" PSITE_MD="$UPDATES_MD" PSITE_MANUAL_SRC="$MANUAL_SRC"
PYOUT=$(python3 <<'PYEOF'
import os, re, sys, io, glob

VER   = os.environ["PSITE_VER"]
VTAG  = "v" + VER  # changelog/官网更新记录对外展示均带 v 前缀（与既有条目/8a 校验一致）
SITE  = os.environ["PSITE_SITE"]
MD    = os.environ["PSITE_MD"]
TSRC  = os.environ["PSITE_MANUAL_SRC"]

def rd(p):
    with io.open(p, encoding="utf-8") as f:
        return f.read()

def wr(p, s):
    with io.open(p, "w", encoding="utf-8", newline="") as f:
        f.write(s)

# ---------- README v$VER 小节解析 ----------
# README 解析基准：SITE 上一级目录下的 README.md（正式=项目根；测试=临时目录根的副本）
readme = rd(os.path.abspath(os.path.join(SITE, "..", "README.md")))
lines = readme.splitlines()
for i, ln in enumerate(lines):
    if ln.startswith("### v%s" % VER):
        j = i + 1
        while j < len(lines) and not lines[j].startswith("### v"):
            j += 1
        sec = lines[i:j]
        break
if sec is None:
    # 测试副本场景：SITE 旁的 README 副本
    alt = os.path.join(os.path.dirname(os.path.abspath(SITE)), "README.md")
    if os.path.isfile(alt):
        for i, ln in enumerate(rd(alt).splitlines()):
            if ln.startswith("### v%s" % VER):
                k = i + 1
                ls = rd(alt).splitlines()
                while k < len(ls) and not ls[k].startswith("### v"):
                    k += 1
                sec = ls[i:k]
                break
if not sec:
    print("README_MISS"); sys.exit(1)

header = sec[0]  # ### v0.6.1（2026-11-27）
m = re.search(r"（([^）]+)）", header)
DATE = m.group(1).strip() if m else ""
TITLE = ""
if m:
    tail = header[m.end():].strip()
    if tail:
        TITLE = tail
if not DATE:
    DATE = __import__("datetime").date.today().isoformat()

body = sec[1:]
# 首条摘要：第一条非空、非列表行
SUMMARY = ""
for ln in body:
    t = ln.strip().rstrip("：:")
    if t and not t.startswith("-"):
        SUMMARY = t
        break
if not SUMMARY:
    SUMMARY = "EasyRDP %s 更新" % VER
# 明细列表：- 标题：明细
items = []
for ln in body:
    t = ln.strip()
    if t.startswith("- ") and not t.startswith("- ["):
        t = t[2:].strip()
        mm = re.split(r"[:：]", t, 1)
        if len(mm) == 2:
            items.append((mm[0].strip(), mm[1].strip()))
        else:
            items.append((t, ""))
print("PARSED ver=%s date=%s summary_len=%d items=%d" % (VER, DATE, len(SUMMARY), len(items)))

# ---------- ① changelog.html ----------
cl_path = os.path.join(SITE, "changelog.html")
cl = rd(cl_path)
if re.search(r"<h2>%s</h2>" % re.escape(VTAG), cl):
    print("CHANGELOG_SKIP 条目已存在")
else:
    lis = "\n".join(
        "          <li><strong>%s</strong>：%s</li>" % (t, d) if d else "          <li>%s</li>" % t
        for t, d in items) or "          <li>%s</li>" % SUMMARY
    block = (
        '      <div class="release">\n'
        '        <div class="release-head">\n'
        '          <h2>%s</h2>\n'
        '          <span class="badge">当前构建</span>\n'
        '          <time datetime="%s">%s</time>\n'
        '        </div>\n'
        '        <p style="color:var(--text-dim);">%s</p>\n\n'
        '        <h3>变更明细</h3>\n        <ul>\n%s\n        </ul>\n      </div>\n'
    ) % (VTAG, DATE, DATE, SUMMARY.replace("\n", " "), lis)
    idx = cl.find('<div class="release">')
    if idx < 0:
        print("CHANGELOG_NO_ANCHOR"); sys.exit(1)
    cl = cl[:idx] + block + "\n" + cl[idx:]
    # 徽章迁移：新块之后（即第一个新块结束之后）的旧「当前构建」徽章降级为 badge-gray + 旧日期
    new_end = cl.find('<h2>%s</h2>' % VTAG)
    old_seg_start = cl.find('<div class="release">', new_end)
    rest = cl[old_seg_start:]
    tm = re.search(r'<time datetime="([^"]+)"', rest)
    old_date = tm.group(1) if tm else DATE
    rest_new = rest.replace('<span class="badge">当前构建</span>',
                            '<span class="badge badge-gray">%s</span>' % old_date, 1)
    cl = cl[:old_seg_start] + rest_new
    wr(cl_path, cl)
    print("CHANGELOG_OK 已插入 %s 条目并迁移徽章（旧块徽章日期 %s）" % (VER, old_date))

# ---------- ② download.html 描述文案 ----------
dl_path = os.path.join(SITE, "download.html")
dl = rd(dl_path)
pat = re.compile(
    r'(<p class="section-desc">当前版本 <strong data-version>[^<]*</strong>（[^）]*）：)'  # 前缀（含版本串，原样保留）
    r'(.*?)(?=(。?macOS / Linux|</p>))', re.S)  # 描述位 → macOS 兜底句或段尾
mm = pat.search(dl)
if mm:
    DESC = SUMMARY.rstrip("。")
    dl = dl[:mm.start(2)] + DESC + dl[mm.end(2):]
    wr(dl_path, dl)
    print("DOWNLOAD_OK 描述已写入: %s" % SUMMARY[:40])
else:
    print("DOWNLOAD_WARN 未命中描述锚点（锚点格式可能变化，请人工核对 download.html section-desc 段）")

# ---------- ③ 官网更新历史记录.md ----------
md = rd(MD)
ev_head = re.compile(r"^## v%s（" % re.escape(VER), re.M)
if ev_head.search(md):
    print("UPDATES_SKIP 本次官网版本条目已存在")
else:
    md_lines = []
    md_lines.append("## v%s（%s）%s 发布同步批次" % (VER, DATE, TITLE or ("v%s 客户端发布" % VER)))
    md_lines.append("")
    SUMMARY_DOT = SUMMARY if SUMMARY.endswith("。") else SUMMARY + "。"
    md_lines.append("- **changelog 同步**：changelog.html 新增 v%s 条目并迁移「当前构建」徽章（%s）" % (VER, SUMMARY_DOT))
    for t, d in items:
        d_dot = d if d.endswith("。") else d + "。"
        md_lines.append("- **%s**：%s" % (t, d_dot))
    md_lines.append("- **download.html 描述更新**：当前版本描述改写为 v%s 批次（%s）" % (VER, SUMMARY_DOT[:80]))
    block = "\n".join(md_lines) + "\n"
    mv = re.search(r"^## v", md, re.M)
    if mv:
        md = md[:mv.start()] + block + "\n" + md[mv.start():]
    else:
        md = md.rstrip("\n") + "\n\n" + block
    wr(MD, md)
    print("UPDATES_OK 已追加官网更新事件 %s（%s）" % (VER, DATE))

# ---------- ④ manual/ 随版维护检测 ----------
tsrc = rd(TSRC)
link_map = re.findall(r'"([A-Za-z0-9_-]+)":\s*"([A-Za-z0-9_-]+\.html)"', tsrc)
manual_dir = os.path.join(SITE, "manual")
pages = set(os.path.basename(p) for p in glob.glob(os.path.join(manual_dir, "*.html")))

def toc_name_for(page):
    # 取既有 toc 中该页显示名（若存在），否则占位名
    for p in glob.glob(os.path.join(manual_dir, "*.html")):
        s = rd(p)
        m2 = re.search(r'<a href="%s">([^<]+)</a>' % re.escape(page), s)
        if m2:
            return m2.group(1)
    return page.replace(".html", "")

created = []
for _eid, page in link_map:
    if page not in pages:
        # 占位页：以 screenshot.html（或任一既有页）为模板
        tpl_src = os.path.join(manual_dir, "screenshot.html")
        if not os.path.isfile(tpl_src):
            cands = sorted(glob.glob(os.path.join(manual_dir, "*.html")))
            tpl_src = cands[0] if cands else None
        if tpl_src is None:
            print("MANUAL_FAIL manual/ 目录为空，无法生成占位页 %s" % page); sys.exit(1)
        tpl = rd(tpl_src)
        name = toc_name_for(page)
        pg = tpl
        pg = re.sub(r"<title>[^<]*</title>", "<title>%s — EasyRDP 用户手册</title>" % name, pg, count=1)
        pg = re.sub(r'(<meta name="description" content=")[^"]*(")',
                    r"\1EasyRDP 用户级操作手册：%s 的入口与用法（随 %s 发布自动生成的占位页，待发布会话补全文案）。\2" % (name, VER), pg, count=1)
        pg = re.sub(r"<h1>[^<]*</h1>", "<h1>%s</h1>" % name, pg, count=1)
        # 正文占位：h1 之后到「相关页面」之前
        ph = ('      <h2 id="entry">功能入口</h2>\n      <p>（占位页：随 %s 发布由 prepare-site.sh 自动生成，'
              '入口与用法说明待发布会话按 MANUAL_LINKS 元素 %s 补全。）</p>\n') % (VER, _eid)
        m3 = re.search(r"(<h1>[^<]*</h1>\n)(.*?)(      <p>相关页面：)", pg, re.S)
        if m3:
            pg = pg[:m3.start(2)] + ph + pg[m3.end(2):]
        wr(os.path.join(manual_dir, page), pg)
        created.append((page, name))
        pages.add(page)
if created:
    print("MANUAL_CREATED " + " ".join("%s(%s)" % c for c in created))
    # toc 注入：既有各页侧栏 + index 总目录
    for p in glob.glob(os.path.join(manual_dir, "*.html")):
        s = rd(p)
        changed = False
        for page, name in created:
            if '<a href="%s"' % page not in s:
                if p.endswith("index.html"):
                    # 总目录表：在第一行 <tr> 之后插入数据行
                    row = '        <tr><td style="padding:6px">%s</td><td style="padding:6px"><a href="%s">%s</a></td><td style="padding:6px">（待发布会话补充入口路径）</td></tr>\n' % (name, page, page)
                    tr = s.find("<tr>")
                    if tr >= 0:
                        tr_end = s.find("\n", tr) + 1
                        s = s[:tr_end] + row + s[tr_end:]
                        changed = True
                elif "doc-toc" in s:
                    s = s.replace("    </aside>", '      <a href="%s">%s</a>\n    </aside>' % (page, name), 1)
                    changed = True
        if changed:
            wr(p, s)
    print("TOC_INJECTED %d 页 toc/index 注入完成" % len(created))

# 入口名称 grep 检测：README 小节「」引用的按钮/入口名
entry_names = set()
for ln in body:
    entry_names.update(re.findall(r"「([^「」]{2,20})」", ln))
todo = []
all_manual = "\n".join(rd(p) for p in glob.glob(os.path.join(manual_dir, "*.html")))
for nm in sorted(entry_names):
    if nm not in all_manual:
        todo.append(nm)
if todo:
    print("MANUAL_TODO " + "|".join(todo))
    sys.exit(2)
print("MANUAL_OK 入口名称全部可在 manual/ 检索到")
PYEOF
)
PYRC=$?
printf '%s\n' "$PYOUT"
if [ "$PYRC" -eq 1 ]; then
  echo "✗ prepare-site 内容手术失败（README 小节缺失或锚点异常），已保留备份 $BK"; exit 1
fi

# ---- site-updates.json 再生成（node 缺失时回退 Windows node.exe，WSL interop）----
NODE_BIN="$(command -v node || command -v node.exe || true)"
if [ "${SKIP_NODE:-0}" != "1" ] && [ -n "$NODE_BIN" ]; then
  # Windows node.exe（WSL interop）需 Windows 形式路径；Linux node 用原路径
  NODE_MD="$UPDATES_MD"; NODE_OUT="$SITE/assets/data/site-updates.json"; NODE_JS="$SITE/tools/gen-site-updates.js"
  case "$NODE_BIN" in
    *node.exe)
      NODE_MD=$(wslpath -w "$UPDATES_MD"); NODE_OUT=$(wslpath -w "$NODE_OUT"); NODE_JS=$(wslpath -w "$NODE_JS") ;;
  esac
  if "$NODE_BIN" "$NODE_JS" "$NODE_MD" "$NODE_OUT"; then
    echo "✓ site-updates.json 已再生成"
  else
    echo "✗ gen-site-updates.js 失败（node 缺失或 md 语法异常），已保留备份 $BK"; exit 1
  fi
else
  echo "（SKIP_NODE=1 跳过 gen-site-updates.js）"
fi

if [ "$PYRC" -eq 2 ]; then
  echo ""
  echo "================ manual 待更新清单（发布会话按此补文案后重跑本脚本）================"
  TODO_LIST=$(printf '%s\n' "$PYOUT" | sed -n 's/^MANUAL_TODO //p')
  OLDIFS=$IFS; IFS='|'
  for t in $TODO_LIST; do echo "  - $t"; done | sort -u
  IFS=$OLDIFS
  echo "  说明：缺页已自动生成占位页并注入 toc/index（见上方 MANUAL_CREATED/TOC_INJECTED）；"
  echo "  上述入口名称在 manual/ 全文检索不到，需补入对应手册页或改写旧文案（改名场景）。"
  echo "===================================================================================="
  exit 2
fi

echo "✓ prepare-site 完成：changelog 条目 + 徽章迁移 + download 描述 + 官网更新记录 + manual 检测"
exit 0

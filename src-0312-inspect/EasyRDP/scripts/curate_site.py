#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
curate_site.py v2 — 发布后官网表格/版本串自动收编（重建式，幂等）。
用法： python3 curate_site.py --ver 0.3.12 --date 2026-11-18 --old 0.3.11 \
         --site <easyrdp-site> --dist <easyrdp-client/dist> --gw <dist-gateway>
逻辑：
  1. download.html / en/download.html：历史表重建为最近 3 条降序
     —— 新版本行（日期=--date，尺寸=dist 实测）+ old 行 + 更早行（沿用原行日期/尺寸）
     被顶替的行（当前表中版本==ver 的行）视为 old 的内容还原。
  2. security.html / en/security.html：顶行 data-version→新版本，次行=old，三行=old-1。
  3. en/*.html（changelog 除外）+ en/index.html JSON-LD：旧版本串→新版本。
"""
import argparse, re, io, os

def human(n):
    return f"{n/1048576.0:.1f} MB"

def reversion(row_html, ver):
    r = row_html
    for oldv in re.findall(r"v0\.3\.\d+", row_html):
        r = r.replace("<td>"+oldv+"</td>", "<td>"+ver+"</td>")
        r = r.replace("<span data-version>"+oldv+"</span>", "<span data-version>"+ver+"</span>")
    r = re.sub(r"EasyRDP-0\.3\.\d+-portable", f"EasyRDP-{ver}-portable", r)
    r = re.sub(r"EasyRDP-0\.3\.\d+-win32-x64-portable", f"EasyRDP-{ver}-win32-x64-portable", r)
    r = re.sub(r"EasyRDP-Gateway-0\.3\.\d+-win-x64", f"EasyRDP-Gateway-{ver}-win-x64", r)
    r = re.sub(r"EasyRDP-src-0\.3\.\d+\.tar\.gz", f"EasyRDP-src-{ver}.tar.gz", r)
    r = re.sub(r"releases/tag/v0\.3\.\d+", f"releases/tag/{ver}", r)
    return r

def fix_download(path, ver, date, dist, gw, old):
    t = io.open(path, encoding="utf-8").read()
    rows = {}   # version -> (date, html)
    order = []
    for m in re.finditer(r"<tr><td>v(0\.3\.\d+)</td><td>(2026-\d\d-\d\d)</td>(.*?)</tr>", t, re.S):
        v, d, body = m.group(1), m.group(2), m.group(3)
        if v == ver and old:
            v = old  # sed 顶替行还原为 old
        html = "<tr><td>v"+v+"</td><td>"+d+"</td>"+body+"</tr>"
        if v not in rows:
            rows[v] = (d, html); order.append(v)
    # 新版本行
    s = (human(os.path.getsize(os.path.join(dist, f"EasyRDP-{ver}-portable.exe"))),
         human(os.path.getsize(os.path.join(dist, f"EasyRDP-{ver}-win32-x64-portable.zip"))),
         human(os.path.getsize(os.path.join(gw,  f"EasyRDP-Gateway-{ver}-win-x64.zip"))),
         human(os.path.getsize(os.path.join(os.path.dirname(gw), f"EasyRDP-src-{ver}.tar.gz"))))
    newhtml = ("<tr><td>v"+ver+"</td><td>"+date+"</td><td><a href=\"downloads/EasyRDP-"+ver+"-portable.exe\">单文件</a>"
               " · <a href=\"downloads/EasyRDP-"+ver+"-win32-x64-portable.zip\">zip</a>"
               " · <a href=\"downloads/EasyRDP-Gateway-"+ver+"-win-x64.zip\">网关</a>"
               " · <a href=\"downloads/EasyRDP-src-"+ver+".tar.gz\">源码</a></td>"
               "<td>"+s[0]+"</td><td>"+s[1]+"</td><td>"+s[2]+"</td><td>"+s[3]+"</td></tr>")
    if "en/" in path.replace(os.sep, "/"):
        newhtml = newhtml.replace("downloads/", "../downloads/").replace(">单文件<", ">Single file<").replace(">zip<", ">zip<").replace(">网关<", ">Gateway<").replace(">源码<", ">Source<")
    # 版本序列：ver 在首，其余按表格出现顺序降序，取 3
    seq = [ver] + [v for v in order if v != ver]
    seq = seq[:3]
    out = []
    for v in seq:
        if v == ver:
            out.append(newhtml + "\n          ")
        elif v in rows:
            out.append(rows[v][1] + "\n          ")
        else:
            # 序列中缺失的中间版本：用骨架（新版本行）reversion，日期沿用 known 或 2026-11-17
            out.append(reversion(newhtml.replace(date, "2026-11-17"), v) + "\n          ")
    t = re.sub(r"(?:[ \t]*<tr><td>v0\.3\.\d+</td>.*?</tr>\s*)+", "".join(out), t, count=1, flags=re.S)
    io.open(path, "w", encoding="utf-8", newline="").write(t)
    tag = "en" if "en" in path.replace(os.sep, "/").split(os.sep) else "zh"
    print(tag, "download:", re.findall(r"<td>(v0\.3\.\d+)</td><td>(2026-\d\d-\d\d)", t))

def fix_security(path, ver, old):
    t = io.open(path, encoding="utf-8").read()
    rows = re.findall(r"<tr><td>(?:<span data-version>)?v0\.3\.\d+(?:</span>)?</td>.*?</tr>", t, re.S)
    cur = rows[0]
    cur = cur.replace("<span data-version>v0.3.8</span>", "<span data-version>v"+ver+"</span>")
    cur = cur.replace("<span data-version>v0.3.9</span>", "<span data-version>v"+ver+"</span>")
    cur = cur.replace("<span data-version>v0.3.10</span>", "<span data-version>v"+ver+"</span>")
    cur = cur.replace("<span data-version>v0.3.11</span>", "<span data-version>v"+ver+"</span>")
    cur = cur.replace("<span data-version>v0.3.12</span>", "<span data-version>v"+ver+"</span>")
    pool = [v for v in re.findall(r"v0\.3\.\d+", "".join(rows)) if v != "v"+ver]
    if old and ("v"+old) not in pool:
        pool.insert(0, "v"+old)
    p1 = pool[0] if pool else "v0.3.0"
    p2 = pool[1] if len(pool) > 1 else "v0.3.0"
    r2 = re.sub(r"v0\.3\.\d+", p1, rows[1])
    r3 = re.sub(r"v0\.3\.\d+", p2, rows[1])
    block = cur + "\n            " + r2 + "\n            " + r3
    t = re.sub(r"(?:[ \t]*<tr><td>(?:<span data-version>)?v0\.3\.\d+(?:</span>)?</td>.*?</tr>\s*)+",
               block + "\n          ", t, count=1, flags=re.S)
    io.open(path, "w", encoding="utf-8", newline="").write(t)
    print(("en " if "en" in path.replace(os.sep, "/").split(os.sep) else "zh") + " security:",
          re.findall(r"v0\.3\.\d+", block))

def fix_en_versions(site, old, ver):
    endir = os.path.join(site, "en")
    for root, dirs, files in os.walk(endir):
        for fn in files:
            if fn.endswith(".html") and fn != "changelog.html":
                p = os.path.join(root, fn)
                t = io.open(p, encoding="utf-8").read()
                if old in t:
                    io.open(p, "w", encoding="utf-8", newline="").write(t.replace(old, ver))
    p = os.path.join(endir, "index.html")
    t = io.open(p, encoding="utf-8").read()
    t = re.sub(r'("softwareVersion":\s*")0\.3\.\d+(")', r"\g<1>" + ver + r"\g<2>", t)
    io.open(p, "w", encoding="utf-8", newline="").write(t)
    print("en versions ->", ver, "| jsonld:", re.findall(r'"softwareVersion": "[^"]+"', t))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ver", required=True)
    ap.add_argument("--date", required=True)
    ap.add_argument("--old", required=True)
    ap.add_argument("--site", required=True)
    ap.add_argument("--dist", required=True)
    ap.add_argument("--gw", required=True)
    a = ap.parse_args()
    fix_download(os.path.join(a.site, "download.html"), a.ver, a.date, a.dist, a.gw, a.old)
    fix_download(os.path.join(a.site, "en", "download.html"), a.ver, a.date, a.dist, a.gw, a.old)
    fix_security(os.path.join(a.site, "security.html"), a.ver, a.old)
    fix_security(os.path.join(a.site, "en", "security.html"), a.ver, a.old)
    fix_en_versions(a.site, a.old, a.ver)

if __name__ == "__main__":
    main()

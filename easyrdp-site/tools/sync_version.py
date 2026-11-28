#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
sync_version.py — 全站版本号统一同步（单一事实来源：assets/js/main.js 的 SITE_VERSION）。

用法: python3 tools/sync_version.py [--check]

功能:
  1. 读取 assets/js/main.js 中 SITE_VERSION 作为唯一版本来源;
  2. 将所有前台 HTML 中 `data-version>vX.Y.Z<` 占位兜底值刷成当前版本;
  3. 修正 en/ 目录正文中的「当前状态」版本串（历史 changelog 条目不动）;
  4. --check 模式: 仅校验不一致即退出码 1（发布流水线守门用）。
"""
import re, sys, io, os

SITE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXCLUDE_DIRS = ("downloads", "api")
EXCLUDE_FILES = ("admin.html",)  # 后台页不展示产品版本

def site_version():
    js = io.open(os.path.join(SITE, "assets", "js", "main.js"), encoding="utf-8").read()
    m = re.search(r'SITE_VERSION\s*=\s*"(v\d+\.\d+\.\d+)"', js)
    if not m:
        sys.exit("FATAL: cannot read SITE_VERSION from assets/js/main.js")
    return m.group(1)

def pages():
    for root, dirs, files in os.walk(SITE):
        dirs[:] = [d for d in dirs if d not in EXCLUDE_DIRS]
        for f in files:
            if f.endswith(".html") and f not in EXCLUDE_FILES:
                yield os.path.join(root, f)

def main():
    ver = site_version()
    check = "--check" in sys.argv
    problems = []
    for path in pages():
        rel = os.path.relpath(path, SITE).replace(os.sep, "/")
        t0 = t = io.open(path, encoding="utf-8").read()

        # 1) data-version 占位兜底值统一
        def repl(m):
            return m.group(1) + ver + m.group(3)
        t = re.sub(r'(data-version>)(v\d+\.\d+\.\d+)(<)', repl, t)

        # 2) en/ 正文「当前状态」版本串（历史条目 h2/h3 及 changelog 页跳过）
        is_en = rel.startswith("en/") and rel != "en/changelog.html"
        if is_en:
            # 仅替换明确表述当前版本的句式,不动历史叙述
            t = re.sub(r'(Current version[^`<]{0,20})`?v\d+\.\d+\.\d+`?', r'\g<1>v' + ver[1:], t)
            t = re.sub(r'(Download v)\d+\.\d+\.\d+', r'\g<1>' + ver[1:], t)
            t = re.sub(r'(MVP \()v\d+\.\d+\.\d+(\))', r'\g<1>' + ver[1:] + r'\g<2>', t)
            t = re.sub(r'(in v)\d+\.\d+\.\d+(, passwords)', r'\g<1>' + ver[1:] + r'\g<2>', t)
            t = re.sub(r'(the )v\d+\.\d+\.\d+( stage)', r'\g<1>' + ver[1:] + r'\g<2>', t)
            t = re.sub(r'(v)\d+\.\d+\.\d+( \.rdp files|, gateway|, ?passwords)', r'\g<1>' + ver[1:] + r'\g<2>', t)

        if t != t0:
            if check:
                problems.append(rel)
            else:
                io.open(path, "w", encoding="utf-8", newline="").write(t)
                print("synced:", rel)

    # 校验:全站不应再有任何与当前版本不一致的 data-version 占位
    for path in pages():
        rel = os.path.relpath(path, SITE).replace(os.sep, "/")
        t = io.open(path, encoding="utf-8").read()
        for m in re.finditer(r'data-version>(v\d+\.\d+\.\d+)<', t):
            if m.group(1) != ver:
                problems.append(f"{rel}: data-version={m.group(1)} != {ver}")

    if problems:
        print("INCONSISTENT:")
        print("\n".join(problems))
        sys.exit(1)
    print(f"OK — all version placeholders consistent with {ver}")

if __name__ == "__main__":
    main()

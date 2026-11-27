# -*- coding: utf-8 -*-
import json, time, shutil, random
DB = "/www/wwwroot/easyrdp-data/feedback-db.json"
shutil.copy(DB, DB + ".bak-simtest-20261126")
db = json.load(open(DB, encoding="utf-8-sig"))
now = int(time.time() * 1000)
random.seed(20261126)

def b36(n):
    digits = "0123456789abcdefghijklmnopqrstuvwxyz"
    s = ""
    while n:
        s = digits[n % 36] + s
        n //= 36
    return s or "0"

items = [
    ("问题反馈", "CTO@trae.email", "远程桌面窗口拖动的时候边缘偶尔还有残影,虽然 v0.2.1 修了很多,但非整数缩放比下拖动窗口边缘偶尔还能看到 1px 旧像素残留,希望能彻底修复"),
    ("功能建议", "CTO@7xm.net", "希望支持直接从 mstsc.exe 的 .rdp 文件批量导入连接配置,手上有几十台机器的连接信息,一条条手动新建太累了,导入导出单文件已经有了,建议加批量导入"),
    ("问题反馈", "CTO@trae.email", "网络抖动断线后重连,会话窗口直接黑屏几秒才恢复画面,而且恢复后剪贴板同步就失效了,要重新连接才能复制粘贴,建议断线重连后自动恢复剪贴板通道"),
    ("功能建议", "CTO@7xm.net", "连接中心建议增加会话分组和标签页功能,现在所有连接平铺在一个列表里,机器多了不好找,希望能按项目/环境分组,并支持多标签页同时开几个会话切换"),
    ("问题反馈", "CTO@trae.email", "本地复制一段比较长的文本粘贴到远程桌面,内容多了以后粘贴会卡一下,而且偶尔远端粘贴出来是乱码,剪贴板大文本同步建议优化"),
    ("功能建议", "CTO@7xm.net", "希望增加连接延迟和丢包率显示,放在会话窗口标题栏或状态栏,远程办公时需要知道当前网络质量,另外建议支持一键截图保存当前远程画面"),
    ("问题反馈", "87512", "启动 EasyRDP 时偶尔会闪一下 PowerShell 黑窗口,虽然 v0.2.1 修复了大部分,但偶发还能看到一闪而过,希望彻底消除"),
    ("功能建议", "", "灾备包功能很好用,建议在客户端里加个入口可以直接导出灾备包,现在要手动跑脚本,方便普通用户备份自己的连接配置"),
]
added = []
for i, (t, c, content) in enumerate(items):
    ts = now - (10 * 60 - i * 61) * 1000
    fid = "fb" + b36(ts) + ("%04x" % random.getrandbits(16))
    mins = time.strftime("%Y-%m-%d %H:%M", time.gmtime(ts / 1000 + 8 * 3600))
    db["items"].append({
        "id": fid, "ts": ts, "date": mins, "ipHash": "simtest%064s" % i,
        "appVersion": "0.2.1", "type": t, "contact": c, "content": content,
        "reply": "", "replyAt": 0, "public": False, "status": "new",
        "systemInfo": {"系统平台": "windows (x86_64)", "软件版本": "0.2.1", "地域": "福建 厦门"},
    })
    added.append("%s(%s)" % (fid, c or "无邮箱"))
json.dump(db, open(DB, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
print("inserted:", len(added))
for a in added:
    print(" ", a)

#!/usr/bin/env bash
# prune-old-downloads.sh —— 删除 198 服务器 downloads 中 0.3.1 之前的旧版本文件，上传新版 download.html，同步 sv
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site

echo "== 上传新版 download.html =="
scp -q -i "$EK" -P 22 "$SITE/download.html" root@198.2.231.95:/www/wwwroot/easyrdp.cn/download.html

echo "== 删除旧版本文件（保留 0.3.1/0.3.2/0.3.3 与 SUMS）=="
ssh -i "$EK" -p 22 root@198.2.231.95 '
  cd /www/wwwroot/easyrdp.cn/downloads
  echo "-- 删除前 --"; ls | wc -l; du -sh .
  rm -f EasyRDP-0.1.* EasyRDP-0.2.[01]-* EasyRDP-0.2.2-* EasyRDP-0.3.0-* EasyRDP-Gateway-0.1.* EasyRDP-Gateway-0.2.[01]-* EasyRDP-Gateway-0.2.2-* EasyRDP-src-0.1.* EasyRDP-src-0.2.[01]-* EasyRDP-src-0.2.2-* EasyRDP-0.3.2-portable.exe.enc EasyRDP-0.3.1-portable.exe.enc 2>/dev/null
  echo "-- 删除后 --"; ls | wc -l; du -sh .
  ls
'

echo "== 上传 download.html 后的权限与同步 sv =="
ssh -i "$EK" -p 22 root@198.2.231.95 'chown www:www /www/wwwroot/easyrdp.cn/download.html; /root/easyrdp-sync-to-sv.sh && tail -1 /var/log/easyrdp-sync.log'
rm -f "$EK"
echo "✅ 清理与部署完成"

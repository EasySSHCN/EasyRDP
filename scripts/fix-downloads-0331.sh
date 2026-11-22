#!/usr/bin/env bash
# fix-downloads-0331.sh —— 清残留 + 补传 0.3.1 四件 + 同步 sv + 线上验证
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
DIST=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-client/dist
GW=dist-gateway

echo "== 清残留 =="
ssh -i "$EK" -p 22 root@198.2.231.95 '
  cd /www/wwwroot/easyrdp.cn/downloads
  rm -f EasyRDP-Gateway-0.3.0-win-x64.zip EasyRDP-src-0.2.0.tar.gz EasyRDP-src-0.2.1.tar.gz EasyRDP-src-0.2.2.tar.gz EasyRDP-src-0.3.0.tar.gz
  ls
'

echo "== 补传 0.3.1 四件 =="
scp -q -i "$EK" -P 22 "$DIST/EasyRDP-0.3.1-portable.exe" root@198.2.231.95:/www/wwwroot/easyrdp.cn/downloads/
scp -q -i "$EK" -P 22 "$DIST/EasyRDP-0.3.1-win32-x64-portable.zip" root@198.2.231.95:/www/wwwroot/easyrdp.cn/downloads/ 2>/dev/null || \
  scp -q -i "$EK" -P 22 "B:/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-client/dist/EasyRDP-0.3.1-win32-x64-portable.zip" root@198.2.231.95:/www/wwwroot/easyrdp.cn/downloads/
scp -q -i "$EK" -P 22 "/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/$GW/EasyRDP-Gateway-0.3.1-win-x64.zip" root@198.2.231.95:/www/wwwroot/easyrdp.cn/downloads/ 2>/dev/null || true
scp -q -i "$EK" -P 22 "/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/EasyRDP-src-0.3.1.tar.gz" root@198.2.231.95:/www/wwwroot/easyrdp.cn/downloads/ 2>/dev/null || true
ssh -i "$EK" -p 22 root@198.2.231.95 'chown www:www /www/wwwroot/easyrdp.cn/downloads/EasyRDP-0.3.1-* 2>/dev/null; ls /www/wwwroot/easyrdp.cn/downloads/'

echo "== 同步 sv =="
ssh -i "$EK" -p 22 root@198.2.231.95 '/root/easyrdp-sync-to-sv.sh && tail -1 /var/log/easyrdp-sync.log'

echo "== 线上验证（0.3.1/0.3.2/0.3.3 单文件 HEAD）=="
for v in 0.3.1 0.3.2 0.3.3; do
  curl -s -o /dev/null -w "EasyRDP-$v-portable.exe %{http_code}\n" -I "https://easyrdp.cn/downloads/EasyRDP-$v-portable.exe"
done
rm -f "$EK"
echo "✅ 完成"

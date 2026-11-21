#!/usr/bin/env bash
# dr-pack-dev-final.sh —— 单进程重打 dev-machine.zip（flock 防重叠）
set -e
exec 9>/var/lock/drpack.lock
flock -n 9 || { echo "已有打包在跑"; exit 0; }
cd /mnt/b/EasyRDP-DR-20261117/dev-machine
rm -f ../dev-machine.zip
zip -r -q ../dev-machine.zip EasyRDP.tar.gz tools install-windows.ps1 install-linux.sh RESTORE-DEV.md
ls -lh ../dev-machine.zip
unzip -t ../dev-machine.zip > /dev/null && echo "ZIP-OK"
echo "✅ dev-machine.zip 完成"

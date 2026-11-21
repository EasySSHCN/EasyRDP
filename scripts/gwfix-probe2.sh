#!/usr/bin/env bash
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn 'systemctl stop easyrdp-gateway && echo service stopped
which cargo rustc git gcc cc 2>/dev/null
ls ~/.cargo/bin 2>/dev/null
cat /etc/os-release | head -2
free -m | head -2
df -h / | tail -1
rm -f /tmp/id_ed25519_easyssh'

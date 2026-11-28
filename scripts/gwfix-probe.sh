#!/usr/bin/env bash
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn 'file /opt/easyrdp-gateway/easyrdp-gateway 2>/dev/null | head -1
ls -la /opt/easyrdp-gateway/
systemctl is-active easyrdp-gateway
journalctl -u easyrdp-gateway -n 6 --no-pager | tail -6
find / -name "easyrdp-gateway*" -not -path "/proc/*" -not -path "/opt/easyrdp-gateway/*" -not -path "/tmp/*" 2>/dev/null | head
rm -f /tmp/id_ed25519_easyssh'

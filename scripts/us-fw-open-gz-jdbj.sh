#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show 'bash -s' <<'REMOTE'
set -e
for ip in 101.96.228.27 117.72.98.174; do
  firewall-cmd --permanent --zone=public --add-rich-rule="rule family=\"ipv4\" source address=\"$ip\" port port=\"22\" protocol=\"tcp\" accept"
  firewall-cmd --zone=public --add-rich-rule="rule family=\"ipv4\" source address=\"$ip\" port port=\"22\" protocol=\"tcp\" accept"
done
echo "== 当前 public 富规则 =="
firewall-cmd --zone=public --list-rich-rules
REMOTE
rm -f "$KE"

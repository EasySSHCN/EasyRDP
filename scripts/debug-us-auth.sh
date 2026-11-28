#!/usr/bin/env bash
set -e
KE=/tmp/kek
KS=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KS"; chmod 600 "$KS"
echo "== us authorized_keys 尾部 =="
ssh -i "$KE" root@us.trae.show 'tail -3 ~/.ssh/authorized_keys; ls -la ~/.ssh | head; sshd -T 2>/dev/null | grep -Ei "permitrootlogin|authorizedkeysfile|pubkeyauth" | head -5'
echo "== sv 侧用详细模式试连 us =="
ssh -i "$KS" -p 53422 root@sv.trae.show 'ssh -v -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@us.trae.show "echo SV-US-OK" 2>&1 | grep -E "Offering|denied|Authentications|SV-US|Permission" | head -8'
rm -f "$KE" "$KS"

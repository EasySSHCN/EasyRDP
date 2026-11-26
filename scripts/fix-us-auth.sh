#!/usr/bin/env bash
set -e
KE=/tmp/kek
KS=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KS"; chmod 600 "$KS"

PUB=$(ssh -i "$KS" -p 53422 -o StrictHostKeyChecking=accept-new root@sv.trae.show 'cat /root/.ssh/id_ed25519.pub')
echo "sv pub: ${PUB:0:30}..."
ssh -i "$KE" root@us.trae.show "python3 - <<'PY'
import io
p='/root/.ssh/authorized_keys'
s=io.open(p).read()
# 撤销上次拼接损坏:把 'sv-pull@sv.trae.show' 后粘上的重复行拆开
bad='sv-pull@sv.trae.showssh-ed25519'
if bad in s:
    s=s.replace(bad,'sv-pull@sv.trae.show\nssh-ed25519')
lines=[l for l in s.splitlines() if l.strip()]
pub='''$PUB'''
if not any(l.split()[1]==pub.split()[1] for l in lines if len(l.split())>=2):
    lines.append(pub)
io.open(p,'w').write('\n'.join(lines)+'\n')
print('FIXED')
PY"
echo "== sv 复测连 us =="
ssh -i "$KS" -p 53422 root@sv.trae.show 'ssh -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@us.trae.show "echo SV-US-OK; hostname"'
rm -f "$KE" "$KS"

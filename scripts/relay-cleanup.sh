#!/usr/bin/env bash
set -e
KE=/tmp/kek
KG=/tmp/kgz
KJ=/tmp/kj
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KG"; chmod 600 "$KG"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KJ"; chmod 600 "$KJ"
echo "== 撤销 us 富规则(gz/jdbj) =="
ssh -i "$KE" root@us.trae.show 'bash -s' <<'REMOTE'
firewall-cmd --permanent --zone=public --remove-rich-rule='rule family="ipv4" source address="101.96.228.27" port port="22" protocol="tcp" accept' || true
firewall-cmd --zone=public --remove-rich-rule='rule family="ipv4" source address="101.96.228.27" port port="22" protocol="tcp" accept' || true
firewall-cmd --permanent --zone=public --remove-rich-rule='rule family="ipv4" source address="117.72.98.174" port port="22" protocol="tcp" accept' || true
firewall-cmd --zone=public --remove-rich-rule='rule family="ipv4" source address="117.72.98.174" port port="22" protocol="tcp" accept' || true
grep -vE '101\.96\.228\.27|117\.72\.98\.174' /root/.ssh/authorized_keys > /tmp/ak && mv /tmp/ak /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys
echo "== 剩余富规则 =="
firewall-cmd --zone=public --list-rich-rules
echo "== 剩余 authorized_keys 行数 =="; wc -l < /root/.ssh/authorized_keys
REMOTE
echo "== 清 gz/jdbj 半截文件 =="
timeout 20 ssh -i "$KG" root@gz.trae.show 'rm -f /root/dr-remote-server-20261122.zip; echo GZ-CLEANED' || true
timeout 20 ssh -i "$KJ" root@jdbj.trae.show 'rm -f /root/dr-remote-server-20261122.zip; echo JDBJ-CLEANED' || true
rm -f "$KE" "$KG" "$KJ"

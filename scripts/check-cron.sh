#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k3
chmod 600 /tmp/k3
H=root@198.2.231.95
ssh -i /tmp/k3 -o StrictHostKeyChecking=accept-new $H '
for f in 3ab48c27ec99cb9787749c362afae517 9f561bf30d8a5ee9bc84cef4b1ad6a90 4e16271d028828d2f2426e209d118d2c 41ffc3dae5519a6ee11757d1e1a3a603; do
  echo "--- $f ---"; head -6 /www/server/cron/$f 2>/dev/null
done
echo "=== bt 面板任务名 ==="
ls /www/server/cron/ | head
'
rm -f /tmp/k3

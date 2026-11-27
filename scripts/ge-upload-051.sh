#!/usr/bin/env bash
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
R=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
D=$R/easyrdp-client/dist
GID=1355624
for F in "$D/EasyRDP-0.5.1-portable.exe" "$D/EasyRDP-0.5.1-win32-x64-portable.zip" \
         "$R/dist-gateway/EasyRDP-Gateway-0.5.1-win-x64.zip" "$R/EasyRDP-src-0.5.1.tar.gz" \
         "$D/SHA256SUMS.txt"; do
  N=$(basename "$F")
  CODE=$(curl -s -o /tmp/ge-up.json -w "%{http_code}" --max-time 3600 \
    -X POST "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$GID/attach_files?access_token=$GE" \
    -F "file=@$F")
  echo "$N -> HTTP $CODE $(head -c 100 /tmp/ge-up.json)"
done

#!/usr/bin/env bash
# release-reupload-062-fix.sh — v0.6.2 重建产物五件套替换 GH Release 461260927 / Gitee 1378535
set -u
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
VT=$ROOT/secrets-vault
D=$ROOT/easyrdp-client/dist
GH=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^github_api_token: //p')
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
[ -n "$GH" ] && [ -n "$GE" ] || { echo "token missing"; exit 1; }

ASSETS=("EasyRDP-0.6.2-portable.exe:$D/EasyRDP-0.6.2-portable.exe" \
"EasyRDP-0.6.2-win32-x64-portable.zip:$D/EasyRDP-0.6.2-win32-x64-portable.zip" \
"EasyRDP-Gateway-0.6.2-win-x64.zip:$ROOT/dist-gateway/EasyRDP-Gateway-0.6.2-win-x64.zip" \
"EasyRDP-src-0.6.2.tar.gz:$ROOT/EasyRDP-src-0.6.2.tar.gz" \
"SHA256SUMS.txt:$D/SHA256SUMS.txt")

GHID=461260927
echo "== GitHub Release $GHID: 删除旧附件 =="
for A in $(curl -s -H "Authorization: token $GH" \
    "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/$GHID/assets" \
    | python3 -c "import json,sys
for a in json.load(sys.stdin): print(a['id'])"); do
  curl -s -X DELETE -H "Authorization: token $GH" \
    "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/assets/$A" -o /dev/null
  echo "  deleted asset $A"
done
echo "== GitHub 上传五件套 =="
for pair in "${ASSETS[@]}"; do
  N="${pair%%:*}"; F="${pair#*:}"
  CODE=$(curl -s -o /tmp/gh-up.json -w "%{http_code}" -X POST \
    -H "Authorization: token $GH" -H "Content-Type: application/octet-stream" \
    --data-binary @"$F" \
    "https://uploads.github.com/repos/EasySSHCN/EasyRDP/releases/$GHID/assets?name=$N")
  echo "  GH $N -> HTTP $CODE $(python3 -c "import json;d=json.load(open('/tmp/gh-up.json'));print(d.get('state') or d.get('message'))" 2>/dev/null)"
done

GID=1378535
echo "== Gitee Release $GID: 删除旧附件 =="
for A in $(curl -s "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$GID/attach_files?access_token=$GE" \
    | python3 -c "import json,sys
for a in json.load(sys.stdin): print(a.get('id') or a.get('browser_download_url',''))"); do
  [ -n "$A" ] && curl -s -X DELETE \
    "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$GID/attach_files/$A?access_token=$GE" -o /dev/null
  echo "  deleted attach $A"
done
echo "== Gitee 上传五件套 =="
for pair in "${ASSETS[@]}"; do
  N="${pair%%:*}"; F="${pair#*:}"
  CODE=$(curl -s -o /tmp/ge-up.json -w "%{http_code}" --max-time 7200 \
    -X POST "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$GID/attach_files?access_token=$GE" \
    -F "file=@$F")
  echo "  GE $N -> HTTP $CODE $(head -c 120 /tmp/ge-up.json)"
done
echo "== 复核 =="
curl -s -H "Authorization: token $GH" "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/$GHID/assets" \
  | python3 -c "import json,sys
for a in json.load(sys.stdin): print('GH', a['name'], a['size'], a['state'])"
curl -s "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$GID?access_token=$GE" \
  | python3 -c "import json,sys
d=json.load(sys.stdin)
for a in (d.get('assets') or []): print('GE', a.get('name'), a.get('size'))"

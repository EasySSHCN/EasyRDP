#!/usr/bin/env bash
# release-gitee-retry-061.sh — 配额清理后重传 v0.6.1 的 4 个失败附件
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
R=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
D=$R/easyrdp-client/dist
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
BASE="https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases"
GID=1375393
echo "== 重传 4 附件 =="
for F in "$D/EasyRDP-0.6.1-portable.exe" "$D/EasyRDP-0.6.1-win32-x64-portable.zip" "$R/dist-gateway/EasyRDP-Gateway-0.6.1-win-x64.zip" "$R/EasyRDP-src-0.6.1.tar.gz"; do
  N=$(basename "$F")
  CODE=$(curl -s -o /tmp/ge-up.json -w "%{http_code}" --max-time 3600 \
    -X POST "$BASE/$GID/attach_files?access_token=$GE" -F "file=@$F")
  echo "$N -> HTTP $CODE $(head -c 160 /tmp/ge-up.json)"
done
curl -s "$BASE/$GID?access_token=$GE" -o /tmp/rel.json
python3 - <<'EOF'
import json
d = json.load(open("/tmp/rel.json"))
print("== 最终附件清单 ==")
print(d.get("tag_name"), "id", d.get("id"))
for a in (d.get("assets") or []):
    print(" -", a.get("name"), a.get("size"))
EOF

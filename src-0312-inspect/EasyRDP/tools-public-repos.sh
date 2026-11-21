#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
echo "== GitHub: set public =="
curl -s -X PATCH -H "Authorization: token $TOKEN" https://api.github.com/repos/EasySSHCN/EasyRDP \
  -d '{"private":false}' | grep -E '"private"|"visibility"|"message"' | head -3
echo "== Gitee: set public =="
curl -s -X PATCH "https://gitee.com/api/v5/repos/easyssh/EasyRDP" \
  -d "access_token=$GITEE" -d "private=false" -d "public=1" \
  | grep -oE '"private":(true|false)' | head -1

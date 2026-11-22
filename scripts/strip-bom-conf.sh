#!/usr/bin/env bash
cd "$(dirname "$0")/.." || exit 1
P=easyrdp-client/src-tauri/tauri.conf.json
python3 - <<'EOF'
import io
p = "easyrdp-client/src-tauri/tauri.conf.json"
s = io.open(p, encoding="utf-8-sig").read()
io.open(p, "w", encoding="utf-8", newline="").write(s)
print("BOM stripped")
EOF
head -c 3 "$P" | od -An -tx1
python3 -m json.tool "$P" > /dev/null && echo JSON_OK

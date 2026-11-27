#!/usr/bin/env bash
cd "$(dirname "$0")/.." || exit 1
python3 - <<'EOF'
import io
p = "scripts/attr-align-005-deploy.sh"
s = io.open(p, encoding="utf-8-sig").read()
io.open(p, "w", encoding="utf-8", newline="").write(s)
print("BOM stripped")
EOF
bash scripts/attr-align-005-deploy.sh

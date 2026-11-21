#!/usr/bin/env bash
set -uo pipefail
cd /opt/dsh/workspace/EasyRDP
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
git push -q -f "https://EasySSHCN:${TOKEN}@github.com/EasySSHCN/EasyRDP.git" ci-workflows:main && echo "WORKFLOWS PUSHED OK" || echo "STILL REJECTED"

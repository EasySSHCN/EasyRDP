#!/usr/bin/env bash
# dr-parallel-pull-and-pack.sh —— 8 路并行分块拉取 + 自动打包灾备压缩包
set -u
KEY=/tmp/kpull
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"
SV="root@sv.trae.show"
OPTS="ssh -i $KEY -p 53422 -o StrictHostKeyChecking=accept-new -o BatchMode=yes"
DR=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build
RS_DIR=$DR/remote-server
WORK=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-pull-work
NWORKERS=8
mkdir -p "$WORK" "$RS_DIR/webroot" "$RS_DIR/easyrdp-data" "$DR/sys-config"
rm -f "$WORK"/chunk-*.list

echo "== [1] 远端文件清单 =="
rsync -r --list-only -e "$OPTS" "$SV:/www/wwwroot/easyrdp.cn/" > "$WORK/list.txt"
echo "  条目: $(wc -l < "$WORK/list.txt")"

echo "== [2] 生成分块清单（轮转分配到 $NWORKERS 路）=="
grep -v '^d' "$WORK/list.txt" | awk '{
  s=$2; gsub(",", "", s);
  name=""; for(i=5;i<=NF;i++) name=name$i" ";
  sub(/ $/,"",name);
  if (name != "" && s + 0 >= 0) print name "\t" s
}' > "$WORK/files.tsv"
n=0
while IFS=$'\t' read -r name size; do
  [ -z "$name" ] && continue
  n=$(( n % NWORKERS + 1 ))
  echo "$name" >> "$WORK/chunk-$n.list"
done < "$WORK/files.tsv"
ls "$WORK"/chunk-*.list

echo "== [3] $NWORKERS 路并行拉取 webroot =="
pids=()
for i in $(seq 1 $NWORKERS); do
  [ -s "$WORK/chunk-$i.list" ] || continue
  rsync -az --partial --files-from="$WORK/chunk-$i.list" -e "$OPTS" "$SV:/www/wwwroot/easyrdp.cn/" "$RS_DIR/webroot/" &
  pids+=($!)
done
fail=0
for p in "${pids[@]}"; do wait "$p" || fail=1; done
if [ $fail -eq 0 ]; then echo "  ✓ webroot 并行拉取完成"; else echo "  ⚠ 有分块失败（重跑可续传）"; fi

echo "== [4] keys.php / 反馈数据 / 系统配置 =="
rsync -az -e "$OPTS" "$SV:/www/wwwroot/easyrdp.cn/api/feedback/keys.php" "$RS_DIR/keys.php"
rsync -az --partial -e "$OPTS" "$SV:/www/wwwroot/easyrdp-data/" "$RS_DIR/easyrdp-data/"
rsync -az -e "$OPTS" "$SV:/etc/nginx/conf.d/" "$DR/sys-config/nginx-conf-sv/" 2>/dev/null || true
rsync -az -e "$OPTS" "$SV:/etc/php-fpm.d/" "$DR/sys-config/php-fpm-sv/" 2>/dev/null || true
rsync -az -e "$OPTS" "root@la.trae.show:/etc/nginx/" "$DR/sys-config/nginx-la/" 2>/dev/null || true

rm -f "$KEY"
echo "== [5] 组装校验 =="
du -sh "$RS_DIR"/webroot "$RS_DIR"/keys.php "$RS_DIR"/easyrdp-data 2>/dev/null

echo "== [6] 打包灾备压缩包 =="
cd /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
tar -czf /mnt/b/EasyRDP-DR-20261117-roles.tar.gz \
  --exclude 'dr-build/dev-machine/tools/EasyRDP/vcpkg/downloads' \
  --exclude 'dr-pull-work' \
  -C dr-build .
ls -lh /mnt/b/EasyRDP-DR-20261117-roles.tar.gz
echo "✅ 并行拉取 + 灾备压缩包全部完成"

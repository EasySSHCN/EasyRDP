#!/usr/bin/env bash
for ip in 140.82.121.3 140.82.121.4 140.82.112.3 140.82.113.3 140.82.114.3 20.205.243.166; do
  curl -s --max-time 10 --resolve github.com:443:$ip -o /dev/null -w "$ip -> %{http_code} %{time_total}s\n" https://github.com/ || echo "$ip fail"
done
getent hosts github.com api.github.com

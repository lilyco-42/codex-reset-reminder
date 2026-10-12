#!/usr/bin/env bash
# 用法：with-retry.sh <总尝试次数> <间隔秒> <命令...>
#
# 为什么要抽出来：ios 那一步会在构建时临时从 GitHub 取 SwiftPM 依赖（Brendonovich/swift-rs），
# runner 上出现过抓取失败导致整条车道红（run 38279394332，2026-10-12）。把 until 循环写在 YAML 里
# 就永远测不到它 —— 而"没测过的重试"等于没有重试：真抖的时候可能压根不生效。
# 抽成脚本后可以由 scripts/test-with-retry.sh 用桩命令把三条分支全跑一遍。
set -uo pipefail

if [ "$#" -lt 4 ]; then
  echo "用法: $0 <总尝试次数> <间隔秒> <命令...>" >&2
  exit 2
fi

max=$1
wait_s=$2
shift 2

if ! [ "$max" -ge 1 ] 2>/dev/null; then
  echo "with-retry: 总尝试次数必须是数字，收到 $max" >&2
  exit 2
fi

attempt=0
while true; do
  attempt=$((attempt + 1))
  if "$@"; then
    if [ "$attempt" -gt 1 ]; then
      echo "with-retry: 第 $attempt 次尝试成功（前面失败 $((attempt - 1)) 次）"
    fi
    exit 0
  fi
  if [ "$attempt" -ge "$max" ]; then
    echo "with-retry: 共尝试 $attempt 次仍失败，判定为真失败" >&2
    exit 1
  fi
  echo "with-retry: 第 $attempt 次失败，等 ${wait_s}s 后重试" >&2
  sleep "$wait_s"
done

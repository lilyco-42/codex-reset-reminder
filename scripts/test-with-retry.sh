#!/usr/bin/env bash
# scripts/with-retry.sh 的自测：三条分支都要跑到，并且断言"实际尝试了几次"这种标量，
# 而不是只看退出码（只看 rc 的话，循环少跑一次或多跑一次都发现不了）。
set -uo pipefail

here=$(dirname "$0")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fails=0
ok() { echo "PASS $1"; }
no() { echo "FAIL $1 — $2"; fails=$((fails + 1)); }

# 1) 第一次就成功：rc=0，且不许打印任何"失败"噪声
out=$(bash "$here/with-retry.sh" 3 0 bash -c 'exit 0' 2>&1); rc=$?
if [ "$rc" = 0 ] && ! printf '%s' "$out" | grep -q "次失败"; then ok "一次成功不打重试噪声 (rc=0)"; else no "一次成功" "rc=$rc out=$out"; fi

# 2) 第一次失败、第二次成功：rc=0，并真的打印了重试与成功在第几次
cat > "$work/flaky.sh" <<'STUB'
#!/usr/bin/env bash
n=$(cat "$COUNTER" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "$COUNTER"
[ "$n" -ge 2 ]
STUB
echo 0 > "$work/count"
out=$(COUNTER="$work/count" bash "$here/with-retry.sh" 3 0 bash "$work/flaky.sh" 2>&1); rc=$?
attempts=$(cat "$work/count")
if [ "$rc" = 0 ] && [ "$attempts" = 2 ] && printf '%s' "$out" | grep -q "第 1 次失败" && printf '%s' "$out" | grep -q "第 2 次尝试成功"; then
  ok "抖一次后成功：实际尝试 $attempts 次，rc=0"
else
  no "抖一次后成功" "rc=$rc attempts=$attempts out=$out"
fi

# 3) 一直失败：rc=1，且尝试次数严格等于上限（不许多烧一次）
cat > "$work/always.sh" <<'STUB'
#!/usr/bin/env bash
n=$(cat "$COUNTER" 2>/dev/null || echo 0)
echo $((n + 1)) > "$COUNTER"
exit 1
STUB
echo 0 > "$work/count3"
out=$(COUNTER="$work/count3" bash "$here/with-retry.sh" 2 0 bash "$work/always.sh" 2>&1); rc=$?
attempts=$(cat "$work/count3")
if [ "$rc" = 1 ] && [ "$attempts" = 2 ] && printf '%s' "$out" | grep -q "共尝试 2 次仍失败"; then
  ok "一直失败：rc=1，尝试 $attempts 次不多烧"
else
  no "一直失败" "rc=$rc attempts=$attempts out=$out"
fi

# 4) 用法错误要立刻拒绝（别把参数当命令跑）
out=$(bash "$here/with-retry.sh" 3 0 2>&1); rc=$?
if [ "$rc" = 2 ]; then ok "参数不足 → rc=2"; else no "参数不足" "rc=$rc out=$out"; fi

echo
if [ "$fails" = 0 ]; then echo "with-retry 自测通过（4 组断言）"; exit 0; fi
echo "$fails 组断言失败"; exit 1

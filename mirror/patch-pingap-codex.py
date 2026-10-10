#!/usr/bin/env python3
"""幂等地给 /etc/pingap.toml 加一条 `/codex` 静态镜像 location。

遵守本仓库 ops 约定（AGENTS.md 第 3 节）的五条不变量：
  1. 幂等：已经是目标状态就退出 0；
  2. 锚点按行精确匹配，0 个或多于 1 个匹配都直接退出非零，绝不猜；
  3. 先验证所有锚点、并 compile()/tomllib 解析即将写盘的完整文本，才开始写；
  4. 写之前留 <file>.bak-<标签>，已存在则不覆盖；
  5. 用 newline='' + splitlines(True) 保留原文件自己的换行符。

默认 --dry-run 语义：不加 --apply 就只打印将要做的改动，绝不写盘。
"""

from __future__ import annotations

import argparse
import os
import shutil
import sys
import tomllib

TARGET = "/etc/pingap.toml"
TAG = "codex-mirror"

# 数组里插入位置：放在最前面，保证早于任何 catch-all（main / sp）被匹配。
ARRAY_ANCHOR = '    "pet",'
ARRAY_LINE = '    "codex_reset",'

SECTIONS = """
[locations.codex_reset]
enable_reverse_proxy_headers = true
path = "/codex"
plugins = ["codex_dir"]
rewrite = "^/codex/?$ /codex/index.html"
upstream = "placeholder"

[plugins.codex_dir]
category = "directory"
charset = "utf-8"
index = "index.html"
path = "/var/www/codex-mirror"
"""


def read_lines(path: str):
    with open(path, "r", encoding="utf-8", newline="") as f:
        raw = f.read()
    eol = "\r\n" if "\r\n" in raw else "\n"
    return raw.splitlines(True), eol


def count_exact(lines, needle: str) -> int:
    return sum(1 for ln in lines if ln.rstrip("\r\n") == needle)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--file", default=TARGET)
    ap.add_argument("--apply", action="store_true", help="真的写盘（默认只 dry-run）")
    args = ap.parse_args()

    if not os.path.isfile(args.file):
        print(f"文件不存在：{args.file}")
        return 2

    lines, eol = read_lines(args.file)
    text = "".join(lines)

    # 幂等：三处目标状态任一已存在就判定为「已打过」，不重复插。
    already = (
        "[locations.codex_reset]" in text
        or "[plugins.codex_dir]" in text
        or ARRAY_LINE in text
    )
    if already:
        if "[locations.codex_reset]" in text and "[plugins.codex_dir]" in text and count_exact(lines, ARRAY_LINE) == 1:
            print("已是目标状态，无需改动（幂等退出）")
            return 0
        print("目标状态不完整（三处只部分存在），拒绝自动修补，请人工核对")
        return 3

    # 锚点必须唯一存在。
    if count_exact(lines, ARRAY_ANCHOR) != 1:
        print(f"锚点 {ARRAY_ANCHOR!r} 出现 {count_exact(lines, ARRAY_ANCHOR)} 次（需要恰好 1 次），中止")
        return 4
    # 不能撞到已有名字。
    for probe in ("[locations.codex_reset]", "[plugins.codex_dir]", "codex_reset"):
        if probe in text:
            print(f"文件里已经有 {probe}，中止")
            return 5

    out = []
    inserted = False
    for ln in lines:
        if not inserted and ln.rstrip("\r\n") == ARRAY_ANCHOR:
            out.append(ARRAY_LINE + eol)
            inserted = True
        out.append(ln)
    if not inserted:
        print("理论不可达：锚点没插入成功")
        return 6

    new_text = "".join(out).rstrip("\r\n") + "\n" + SECTIONS.replace("\n", eol)

    # 写之前先解析即将落盘的完整文本，坏了就不写。
    try:
        parsed = tomllib.loads(new_text.replace("\r\n", "\n"))
    except Exception as e:  # noqa: BLE001
        print(f"即将写盘的 TOML 解析失败，拒绝写入：{e}")
        return 7
    locs = parsed["servers"]["https"]["locations"]
    if "codex_reset" not in locs:
        print("自检失败：locations 里没有 codex_reset")
        return 8
    if locs.index("codex_reset") > locs.index("sp"):
        print("自检失败：codex_reset 排在 sp 之后")
        return 9
    if parsed["locations"]["codex_reset"]["path"] != "/codex":
        print("自检失败：path 不是 /codex")
        return 10
    if "codex_dir" not in parsed["plugins"]:
        print("自检失败：plugins.codex_dir 缺失")
        return 11

    import difflib

    diff = list(
        difflib.unified_diff(
            text.splitlines(), new_text.splitlines(), lineterm="", n=0
        )
    )
    added = [
        ln[1:].rstrip()
        for ln in diff
        if ln.startswith("+") and not ln.startswith("+++")
    ]
    print(f"将新增 {len(added)} 行：")
    for d in added:
        if d:
            print("  +", d)

    if not args.apply:
        print("\ndry-run 完成，未写盘。确认无误后加 --apply")
        return 0

    bak = f"{args.file}.bak-{TAG}"
    if os.path.exists(bak):
        print(f"已存在备份 {bak}，不覆盖（如需重来请人工处理）")
    else:
        shutil.copy2(args.file, bak)
        print(f"备份写入 {bak}")

    tmp = args.file + f".new-{TAG}"
    with open(tmp, "w", encoding="utf-8", newline="") as f:
        f.write(new_text)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, args.file)
    print("已写入。下一步（要人工确认再执行，reload 有 1–3 秒抖动）：")
    print("  pingap -c /etc/pingap.toml -t      # 必须先通过")
    print("  systemctl reload pingap            # 或按你们现有的 reload 方式")
    print(f"回滚：cp {bak} {args.file} && systemctl reload pingap")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env node
// 审计 package-lock.json 里"有节点却没版本号"的条目。
//
// 为什么要这么一条：npm 11 给 rolldown 的平台绑定（@rolldown/binding-darwin-x64）写出过
// `{"dev":true,"optional":true}` 这种没有 version/resolved 的条目，本机 `npm ci --dry-run` 在
// win32 上过得去，CI 的 linux runner 却直接 `npm error Invalid Version: ` 崩在解析阶段，
// 而且报错不带任何上下文（run 38578108239 / 38579716734 两次都是它，第二次已经换成 node 24 + npm 11 仍然红）。
// 这条检查把那种锁在推送前就拦下来，而不是让四条车道各撞一次。
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import process from "node:process";

const here = fileURLToPath(new URL(".", import.meta.url));
const lockPath = process.argv[2] || here + "../package-lock.json";
const lock = JSON.parse(readFileSync(lockPath, "utf8"));

if (lock.lockfileVersion !== 3) {
  console.error(`锁格式不是 lockfileVersion 3，而是 ${lock.lockfileVersion}`);
  process.exitCode = 1;
}

const bad = Object.entries(lock.packages || {}).filter(
  ([name, entry]) => name && !entry?.version
);
if (bad.length) {
  console.error(`锁里有 ${bad.length} 个条目没有版本号（npm ci 会在解析阶段崩）：`);
  for (const [name, entry] of bad.slice(0, 10)) {
    console.error(`  ${name}  ${JSON.stringify(entry).slice(0, 120)}`);
  }
  process.exitCode = 1;
} else {
  const n = Object.keys(lock.packages || {}).length;
  console.log(`PASS 锁审计：${n} 个条目全部带版本号，lockfileVersion=${lock.lockfileVersion}`);
}

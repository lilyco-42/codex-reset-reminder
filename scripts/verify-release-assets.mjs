// 核对发布产物：逐个下载、检查 HTTP 状态、体积、sha256 与文件签名。
// 用法：
//   node scripts/verify-release-assets.mjs --release v0.1.0
//   node scripts/verify-release-assets.mjs --run 38060907917
// 目的：不接受"车道绿了"这种间接证据，只看文件本身能不能下下来、是不是真的安装包。
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";

const REPO = process.env.CRR_REPO || "lilyco-42/codex-reset-reminder";
const API = "https://api.github.com";

const args = process.argv.slice(2);
function arg(name) {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : null;
}
const releaseTag = arg("--release");
const runId = arg("--run");
if (!releaseTag && !runId) {
  console.error("需要 --release <tag> 或 --run <runId>");
  process.exit(2);
}

const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
if (!token) {
  console.error("缺少 GITHUB_TOKEN / GH_TOKEN");
  process.exit(2);
}

const SIGS = {
  ".exe": [["MZ", 0x4d, 0x5a]],
  ".msi": [["OLE2", 0xd0, 0xcf, 0x11, 0xe0]],
  ".dmg": [["zlib", 0x78], ["koly-tail", null]],
  ".appimage": [["ELF", 0x7f, 0x45, 0x4c, 0x46]],
  ".deb": [["ar!", 0x21, 0x3c, 0x61, 0x72]],
  ".apk": [["ZIP", 0x50, 0x4b, 0x03, 0x04]],
  ".ipa": [["ZIP", 0x50, 0x4b, 0x03, 0x04]],
};

function ext(name) {
  const i = name.lastIndexOf(".");
  return i < 0 ? "" : name.slice(i).toLowerCase();
}

function signatureOk(buf, extension) {
  const rules = SIGS[extension];
  if (!rules) return { ok: true, note: "无签名规则" };
  for (const [label, ...bytes] of rules) {
    if (label === "koly-tail") {
      const tail = buf.subarray(buf.length - 512, buf.length - 512 + 4).toString("latin1");
      if (tail === "koly") return { ok: true, note: "koly UDIF 尾部" };
      continue;
    }
    let match = true;
    for (let i = 0; i < bytes.length; i++) if (buf[i] !== bytes[i]) match = false;
    if (match) return { ok: true, note: label };
  }
  return { ok: false, note: "签名不符，前 4 字节 " + buf.subarray(0, 4).toString("hex") };
}

async function gh(path) {
  const res = await fetch(API + path, {
    headers: { Authorization: `Bearer ${token}`, "User-Agent": "crr-verify", Accept: "application/vnd.github+json" },
  });
  if (!res.ok) throw new Error(`${path} -> HTTP ${res.status}`);
  return res.json();
}

let targets = [];
if (releaseTag) {
  const rel = await gh(`/repos/${REPO}/releases/tags/${releaseTag}`);
  console.log(`Release ${rel.tag_name}  draft=${rel.draft} prerelease=${rel.prerelease}  附件数=${rel.assets.length}`);
  targets = rel.assets.map((a) => ({
    name: a.name,
    url: a.url,
    browser_url: a.browser_download_url,
    expected: a.size,
    api: true,
  }));
} else {
  const list = await gh(`/repos/${REPO}/actions/runs/${runId}/artifacts`);
  targets = list.artifacts
    .filter((a) => !a.expired && a.name !== "probe-report")
    .map((a) => ({ name: a.name, id: a.id, expected: a.size_in_bytes }));
}

if (!targets.length) {
  console.error("没有任何产物可核对");
  process.exit(1);
}

const report = [];
let failures = 0;

for (const t of targets) {
  if (releaseTag) {
    const res = await fetch(t.url, {
      headers: { Authorization: `Bearer ${token}`, Accept: "application/octet-stream", "User-Agent": "crr-verify" },
      redirect: "follow",
    });
    const buf = Buffer.from(await res.arrayBuffer());
    const sha = createHash("sha256").update(buf).digest("hex");
    const sig = signatureOk(buf, ext(t.name));
    // 公开仓库的附件必须能匿名下载，否则别人拿到的链接是死的。
    const anon = await fetch(t.browser_url, {
      headers: { "User-Agent": "crr-verify-anon" },
      redirect: "follow",
      method: "GET",
      range: "bytes=0-63",
    });
    const anonBuf = Buffer.from(await anon.arrayBuffer());
    const anonOk = (anon.status === 200 || anon.status === 206) && anonBuf.length > 0;
    const ok = res.status === 200 && buf.length > 0 && buf.length === t.expected && sig.ok && anonOk;
    if (!ok) failures++;
    report.push({
      name: t.name,
      http: res.status,
      bytes: buf.length,
      expected: t.expected,
      sha256: sha,
      sig: sig.note,
      anon_http: anon.status,
      anon_bytes: anonBuf.length,
      download_url: t.browser_url,
      ok,
    });
    console.log(
      `${ok ? "PASS" : "FAIL"} ${t.name.padEnd(52)} http=${res.status} ${buf.length}/${t.expected} anon=${anon.status} ${sig.note}`
    );
  } else {
    // 运行产物动辄几十 MB，这里只用 Range 取头部验证可下载性与签名，
    // 体积以 API 声明值为准；逐字节核对留给真正的 Release 附件。
    const res = await fetch(`${API}/repos/${REPO}/actions/artifacts/${t.id}/zip`, {
      headers: { Authorization: `Bearer ${token}`, "User-Agent": "crr-verify", Range: "bytes=0-63" },
      redirect: "follow",
    });
    const buf = Buffer.from(await res.arrayBuffer());
    const sigOk = buf.subarray(0, 2).toString("latin1") === "PK";
    const ok = (res.status === 200 || res.status === 206) && buf.length > 0 && sigOk;
    if (!ok) failures++;
    report.push({ name: t.name, http: res.status, head_bytes: buf.length, expected: t.expected, zip_sig: sigOk, ok });
    console.log(`${ok ? "PASS" : "FAIL"} ${t.name.padEnd(52)} http=${res.status} head=${buf.length}B 声明体积=${t.expected} PK=${sigOk}`);
  }
}

writeFileSync(new URL("../asset-report.json", import.meta.url), JSON.stringify(report, null, 2));
console.log(`\n${targets.length - failures}/${targets.length} 通过，明细见 asset-report.json`);
process.exit(failures ? 1 : 0);

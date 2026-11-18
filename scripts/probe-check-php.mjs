// probe-check-php.mjs —— 端到端复现客户端 check_update：信封请求→解密→重建规范串→验签
import crypto from "node:crypto";

const MASTER = Buffer.from("qlrrhcw2cUk7Zqq1gMhg/AQaeyilB/NblUVibUGb0yQ=", "base64");
const encKey = crypto.createHmac("sha256", MASTER).update("update-enc").digest();
const macKey = crypto.createHmac("sha256", MASTER).update("update-mac").digest();

function buildEnvelope(plain) {
  const ts = String(Date.now());
  const nonce = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv("aes-256-gcm", encKey, nonce);
  const ct = Buffer.concat([cipher.update(plain, "utf8"), cipher.final()]);
  const tag = cipher.getAuthTag();
  const nonceB = nonce.toString("base64"), ctB = ct.toString("base64"), tagB = tag.toString("base64");
  const mac = crypto.createHmac("sha256", macKey).update(`${ts}.${nonceB}.${ctB}.${tagB}`).digest("base64");
  return JSON.stringify({ v: 1, alg: "A256GCM-HS256", ts: Number(ts), nonce: nonceB, ct: ctB, tag: tagB, mac });
}

const envelope = buildEnvelope(JSON.stringify({ currentVersion: "0.3.0", os: "windows", arch: "x86_64" }));
const res = await fetch("https://easyrdp.cn/api/update/check.php", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: envelope,
});
const raw = await res.text();
console.log("HTTP", res.status, "bytes", raw.length);
const env = JSON.parse(raw);
// 校验 mac
const want = crypto.createHmac("sha256", macKey)
  .update(`${env.ts}.${env.nonce}.${env.ct}.${env.tag}`).digest("base64");
console.log("mac ok:", want === env.mac);
// GCM 解密
const decipher = crypto.createDecipheriv("aes-256-gcm", encKey, Buffer.from(env.nonce, "base64"));
decipher.setAuthTag(Buffer.from(env.tag, "base64"));
const plain = Buffer.concat([decipher.update(Buffer.from(env.ct, "base64")), decipher.final()]).toString("utf8");
console.log("plain bytes:", Buffer.byteLength(plain));
const manifest = JSON.parse(plain);
console.log("fields:", Object.keys(manifest).join(","), "latest:", manifest.latestVersion);

// 客户端同款规范串重建 + 验签
const canonical = JSON.stringify({
  latestVersion: manifest.latestVersion,
  file: manifest.file,
  notes: manifest.notes,
  sha256: manifest.sha256,
});
const pubRaw = Buffer.from("YI9WJ+46MBjo4RBBpJBrqY9tL1q8pfXV2oTa7y1iEos=", "base64");
const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), pubRaw]);
const keyObj = crypto.createPublicKey({ key: spki, format: "der", type: "spki" });
const sig = Buffer.from(manifest.sig, "base64");
const ok = crypto.verify(null, Buffer.from(canonical, "utf8"), keyObj, sig);
console.log("sig verify (envelope path):", ok ? "VALID" : "INVALID");
if (!ok) {
  // 与静态 latest.json 规范串对比
  const fs = await import("node:fs");
  const lm = JSON.parse(fs.readFileSync("C:/Users/ZhenAn/AppData/Local/Temp/ota-latest.json", "utf8"));
  const lc = JSON.stringify({ latestVersion: lm.latestVersion, file: lm.file, notes: lm.notes, sha256: lm.sha256 });
  console.log("envelope canonical bytes:", Buffer.byteLength(canonical), "static canonical bytes:", Buffer.byteLength(lc));
  console.log("identical:", canonical === lc);
  for (const k of ["latestVersion", "file", "sha256"]) {
    if (manifest[k] !== lm[k]) console.log(`DIFF ${k}:`, JSON.stringify(manifest[k]), "vs", JSON.stringify(lm[k]));
  }
  if (manifest.notes !== lm.notes) {
    console.log("DIFF notes lengths:", Buffer.byteLength(manifest.notes), "vs", Buffer.byteLength(lm.notes));
    for (let i = 0; i < Math.min(manifest.notes.length, lm.notes.length); i++) {
      if (manifest.notes[i] !== lm.notes[i]) { console.log("first diff at", i, JSON.stringify(manifest.notes.slice(i, i + 8)), "vs", JSON.stringify(lm.notes.slice(i, i + 8))); break; }
    }
  }
}

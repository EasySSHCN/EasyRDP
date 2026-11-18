// verify-ota.mjs —— 独立验签探针：node 语义重建规范串 + Ed25519 验签
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import crypto from "node:crypto";

const manifest = JSON.parse(readFileSync("C:/Users/ZhenAn/AppData/Local/Temp/ota-latest.json", "utf8"));
const canonical = JSON.stringify({
  latestVersion: manifest.latestVersion,
  file: manifest.file,
  notes: manifest.notes,
  sha256: manifest.sha256,
});
const pubRaw = Buffer.from("YI9WJ+46MBjo4RBBpJBrqY9tL1q8pfXV2oTa7y1iEos=", "base64");
// Raw32 → DER SPKI（Ed25519 固定前缀 302a300506032b6570032100）
const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), pubRaw]);
const sig = Buffer.from(manifest.sig, "base64");
const keyObj = crypto.createPublicKey({ key: spki, format: "der", type: "spki" });
const ok = crypto.verify(null, Buffer.from(canonical, "utf8"), keyObj, sig);
console.log("canonical bytes:", Buffer.byteLength(canonical, "utf8"));
console.log("notes bytes:", Buffer.byteLength(manifest.notes, "utf8"));
console.log("sig verify (node semantics):", ok ? "VALID" : "INVALID");
// 附：明文 sha256 自检（需 .enc 同目录，跳过时提示）
if (process.argv[2]) {
  const h = createHash("sha256").update(readFileSync(process.argv[2])).digest("hex");
  console.log("enc sha256:", h, h === manifest.sha256 ? "== manifest(注意:清单是明文哈希,应为不相等)" : "!= manifest(正常,清单为明文哈希)");
}

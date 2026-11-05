// 上架材料自检：字段是否符合 dbx-store 的 candidate schema，产物清单是否完整。
// 用法：node scripts/check-listing.js（先跑 scripts/package.js，它读 dist/）

const fs = require("fs");

const candidatePath = "dist/candidates/com.eyecare.dbx.json";
const releasePath = "dist/release-candidates.json";
const allowed = new Set([
  "schemaVersion", "id", "publisher", "version", "name", "description", "icon",
  "tags", "permissions", "source", "homepage", "license", "releaseNotes",
  "localizations", "targets",
]);
const required = ["schemaVersion", "id", "publisher", "version", "targets"];
const errors = [];

function checkTargets(label, list) {
  if (!Array.isArray(list) || list.length === 0) {
    errors.push(`${label}: targets 为空`);
    return;
  }
  for (const [i, t] of list.entries()) {
    for (const key of ["target", "url", "sha256", "size"]) {
      if (!(key in t)) errors.push(`${label}: targets[${i}] 缺 ${key}`);
    }
    for (const key of Object.keys(t)) {
      if (!["target", "url", "sha256", "size"].includes(key)) {
        errors.push(`${label}: targets[${i}] 多余字段 ${key}`);
      }
    }
    if (!/^[a-z0-9-]+$/.test(t.target)) errors.push(`${label}: targets[${i}].target 不合规`);
    if (!/^https:\/\//.test(t.url)) errors.push(`${label}: targets[${i}].url 不是 https`);
    if (!/^[A-Fa-f0-9]{64}$/.test(t.sha256)) errors.push(`${label}: targets[${i}].sha256 格式错`);
    if (!(Number.isInteger(t.size) && t.size > 0 && t.size <= 536870912)) {
      errors.push(`${label}: targets[${i}].size 越界`);
    }
  }
}

// 提 PR 用的候选：严格按 CONTRIBUTING 的 schema
{
  const c = JSON.parse(fs.readFileSync(candidatePath, "utf8"));
  for (const key of required) if (!(key in c)) errors.push(`${candidatePath}: 缺字段 ${key}`);
  for (const key of Object.keys(c)) if (!allowed.has(key)) errors.push(`${candidatePath}: 多余字段 ${key}`);

  if (c.schemaVersion !== 1) errors.push(`${candidatePath}: schemaVersion 必须是 1`);
  if (!/^[a-z0-9][a-z0-9._-]*$/.test(c.id)) errors.push(`${candidatePath}: id 不合规`);
  if (!/^[a-z0-9][a-z0-9._-]*$/.test(c.publisher)) errors.push(`${candidatePath}: publisher 不合规`);
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$/.test(c.version)) {
    errors.push(`${candidatePath}: version 不是语义化版本`);
  }
  if (!/^https:\/\//.test(c.icon)) errors.push(`${candidatePath}: icon 不是 https`);
  if (!/^https:\/\//.test(c.source)) errors.push(`${candidatePath}: source 不是 https`);
  checkTargets(candidatePath, c.targets);
}

// 挂 Release 的聚合文件：格式不同，只查关键字段和产物清单
{
  const doc = JSON.parse(fs.readFileSync(releasePath, "utf8"));
  const p = doc.plugin || {};
  for (const key of ["id", "publisher", "version", "name"]) {
    if (!p[key]) errors.push(`${releasePath}: plugin 缺 ${key}`);
  }
  checkTargets(releasePath, doc.artifacts);
}

if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
console.log("两份上架文件都符合预期");

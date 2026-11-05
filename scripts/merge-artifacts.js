// 把各平台的产物合成一份完整的上架材料。
//
// 用在 CI：矩阵里每个平台各自打一个包，上传成 artifact；这个脚本把
// artifacts/<target>/ 下的包全收进来，一次生成
//   dist/release-candidates.json
//   dist/candidates/<id>.json
//   dist/publishers/<publisher>.json
//
// 官方 CLI 不允许跨平台打包，所以「一次出全」只能靠这种方式，
// 而不是在一台机器上连着跑五遍。

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const { buildCandidate, buildRelease, sortTargets } = require("./listing");

const ROOT = path.resolve(__dirname, "..");
const DIST = path.join(ROOT, "dist");
const ARTIFACTS = path.join(ROOT, "artifacts");
const REPO = "https://github.com/xyun2028/dbx-eyecare";
const RAW = "https://raw.githubusercontent.com/xyun2028/dbx-eyecare";

const EXPECTED_TARGETS = [
  "windows-x64",
  "darwin-x64",
  "darwin-arm64",
  "linux-x64",
  "linux-arm64",
];

const sha256 = (file) => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));
const writeJson = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, 2) + "\n");

function main() {
  if (!fs.existsSync(ARTIFACTS)) {
    throw new Error(`没有找到 ${ARTIFACTS}，先让 CI 把各平台的产物下载下来`);
  }

  const manifest = readJson(path.join(ROOT, "manifest.json"));
  const store = fs.existsSync(path.join(ROOT, ".dbx-store.json"))
    ? readJson(path.join(ROOT, ".dbx-store.json"))
    : {};
  const tag = `v${manifest.version}`;

  const entries = [];
  fs.mkdirSync(DIST, { recursive: true });

  for (const name of fs.readdirSync(ARTIFACTS)) {
    const dir = path.join(ARTIFACTS, name);
    if (!fs.statSync(dir).isDirectory()) continue;

    const files = fs.readdirSync(dir);
    const pkg = files.find((file) => file.endsWith(".dbxp"));
    const meta = files.find((file) => file.endsWith(".artifact.json"));
    if (!pkg || !meta) {
      console.log(`跳过 ${name}：没有包或没有 artifact`);
      continue;
    }

    const info = readJson(path.join(dir, meta));
    const actual = sha256(path.join(dir, pkg));
    if (actual !== info.sha256) {
      throw new Error(`${name}: artifact 记录的 sha256 与包不符`);
    }
    if (entries.some((entry) => entry.target === info.target)) {
      throw new Error(`target 重复：${info.target}`);
    }

    entries.push({
      target: info.target,
      url: `${REPO}/releases/download/${tag}/${pkg}`,
      sha256: info.sha256,
      size: info.size,
    });

    fs.copyFileSync(path.join(dir, pkg), path.join(DIST, pkg));
    fs.copyFileSync(path.join(dir, meta), path.join(DIST, meta));
    console.log(`收入 ${info.target}  ${pkg}  ${(info.size / 1024 / 1024).toFixed(2)} MB`);
  }

  if (entries.length === 0) {
    throw new Error("一个包都没找到");
  }

  const artifacts = sortTargets(entries);

  writeJson(
    path.join(DIST, "release-candidates.json"),
    buildRelease({ manifest, store, artifacts })
  );

  fs.mkdirSync(path.join(DIST, "candidates"), { recursive: true });
  writeJson(
    path.join(DIST, "candidates", `${manifest.id}.json`),
    buildCandidate({ manifest, store, artifacts, repo: REPO, rawBase: RAW, tag })
  );

  fs.mkdirSync(path.join(DIST, "publishers"), { recursive: true });
  writeJson(path.join(DIST, "publishers", `${manifest.publisher}.json`), {
    id: manifest.publisher,
    name: manifest.publisher,
    status: "unverified",
  });

  const missing = EXPECTED_TARGETS.filter(
    (target) => !artifacts.some((item) => item.target === target)
  );

  console.log(`\n版本 ${manifest.version}，已覆盖 ${artifacts.length} 个平台：`);
  for (const item of artifacts) console.log(`  ${item.target}`);
  if (missing.length) {
    console.log(`还缺：${missing.join(", ")}`);
    console.log("缺平台不算错，但上架前要想清楚是不是本来就不打算支持");
  }
  console.log("\n上架材料：");
  console.log("  dist/release-candidates.json");
  console.log(`  dist/candidates/${manifest.id}.json`);
  console.log(`  dist/publishers/${manifest.publisher}.json`);
}

main();

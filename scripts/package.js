// Eye眼 DBX 插件打包脚本。
//
// DBX 安装器有三条硬约定，少一条就装不上，而且不会告诉你为什么：
//   1. manifest 的 backend.executable 必须指向包内真实文件，布局是 bin/<target>/<binary>
//   2. 包根必须有 checksums.json，且精确覆盖除它自己和 signature.json 之外的所有文件
//   3. zip 条目要带 Unix 权限位：bin 下 755，其余 644，version-made-by 的 OS 标为 Unix
//
// 所以这里干的事是：构建 → 暂存 → 算哈希 → 写 zip（带权限位）→ 重新打开 zip 自检。
// 自检不是走过场，覆盖不精确这类错误在宿主那边只会得到一句 incompatible。
//
// 用法：
//   node scripts/package.js                # 构建并打包
//   node scripts/package.js --skip-build   # 用已有的 release 产物
//   node scripts/package.js --target linux-x64

const fs = require("fs");
const path = require("path");
const zlib = require("zlib");
const crypto = require("crypto");
const { execFileSync } = require("child_process");

const ROOT = path.resolve(__dirname, "..");
const DIST = path.join(ROOT, "dist");
const STAGE = path.join(DIST, "stage");

const args = process.argv.slice(2);
const skipBuild = args.includes("--skip-build");
const targetArg = args.indexOf("--target");
const TARGET = targetArg >= 0 ? args[targetArg + 1] : defaultTarget();

function defaultTarget() {
  const os =
    process.platform === "win32" ? "windows" : process.platform === "darwin" ? "darwin" : "linux";
  const arch = process.arch === "arm64" ? "arm64" : "x64";
  return `${os}-${arch}`;
}

const isWindows = TARGET.startsWith("windows");
const BIN_NAME = isWindows ? "eyecare-sidecar.exe" : "eyecare-sidecar";

/* ------------------------------------------------------------------ 工具 */

function rmrf(dir) {
  fs.rmSync(dir, { recursive: true, force: true });
}

function copyTree(src, dest) {
  const stat = fs.statSync(src);
  if (stat.isDirectory()) {
    fs.mkdirSync(dest, { recursive: true });
    for (const name of fs.readdirSync(src)) copyTree(path.join(src, name), path.join(dest, name));
  } else {
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.copyFileSync(src, dest);
  }
}

function walk(dir, base = dir) {
  const out = [];
  for (const name of fs.readdirSync(dir)) {
    const full = path.join(dir, name);
    if (fs.statSync(full).isDirectory()) out.push(...walk(full, base));
    else out.push(path.relative(base, full).split(path.sep).join("/"));
  }
  return out;
}

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

const crcTable = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  return table;
})();

function crc32(buf) {
  let c = -1;
  for (let i = 0; i < buf.length; i++) c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}

// 固定时间戳，打包结果可复现；宿主不关心 mtime
const DOS_TIME = 0;
const DOS_DATE = ((2026 - 1980) << 9) | (1 << 5) | 1;

/* -------------------------------------------------------------- zip 写入 */

function writeZip(entries, outFile) {
  const locals = [];
  const centrals = [];
  let offset = 0;

  for (const entry of entries) {
    const nameBuf = Buffer.from(entry.name, "utf8");
    const raw = fs.readFileSync(entry.path);
    const data = zlib.deflateRawSync(raw, { level: 9 });
    const crc = crc32(raw);

    // 0x0800 = 文件名用 UTF-8
    const flags = 0x0800;
    const method = 8;

    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(flags, 6);
    local.writeUInt16LE(method, 8);
    local.writeUInt16LE(DOS_TIME, 10);
    local.writeUInt16LE(DOS_DATE, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(raw.length, 22);
    local.writeUInt16LE(nameBuf.length, 26);
    local.writeUInt16LE(0, 28);

    locals.push(local, nameBuf, data);

    // 高字节 3 = Unix，宿主靠它决定要不要认权限位
    const mode = entry.executable ? 0o100755 : 0o100644;

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE((3 << 8) | 20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(flags, 8);
    central.writeUInt16LE(method, 10);
    central.writeUInt16LE(DOS_TIME, 12);
    central.writeUInt16LE(DOS_DATE, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(raw.length, 24);
    central.writeUInt16LE(nameBuf.length, 28);
    central.writeUInt16LE(0, 30);
    central.writeUInt16LE(0, 32);
    central.writeUInt16LE(0, 34);
    central.writeUInt16LE(0, 36);
    central.writeUInt32LE((mode << 16) >>> 0, 38);
    central.writeUInt32LE(offset, 42);

    centrals.push(central, nameBuf);
    offset += local.length + nameBuf.length + data.length;
  }

  const centralBuf = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4);
  eocd.writeUInt16LE(0, 6);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(centralBuf.length, 12);
  eocd.writeUInt32LE(offset, 16);
  eocd.writeUInt16LE(0, 20);

  fs.writeFileSync(outFile, Buffer.concat([...locals, centralBuf, eocd]));
}

/* -------------------------------------------------------------- zip 自检 */

function listZip(file) {
  const buf = fs.readFileSync(file);

  let eocd = -1;
  for (let i = buf.length - 22; i >= 0; i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new Error("不是合法的 zip：找不到 EOCD");

  const count = buf.readUInt16LE(eocd + 10);
  let ptr = buf.readUInt32LE(eocd + 16);
  const names = [];

  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(ptr) !== 0x02014b50) throw new Error("中央目录损坏");
    const nameLen = buf.readUInt16LE(ptr + 28);
    const extraLen = buf.readUInt16LE(ptr + 30);
    const commentLen = buf.readUInt16LE(ptr + 32);
    const external = buf.readUInt32LE(ptr + 38);
    const name = buf.toString("utf8", ptr + 46, ptr + 46 + nameLen);
    names.push({ name, mode: external >>> 16 });
    ptr += 46 + nameLen + extraLen + commentLen;
  }
  return names;
}

/* ---------------------------------------------------------------- 主流程 */

function main() {
  const manifest = JSON.parse(fs.readFileSync(path.join(ROOT, "manifest.json"), "utf8"));
  console.log(`打包 ${manifest.name} v${manifest.version} → ${TARGET}`);

  if (!skipBuild) {
    console.log("构建侧车（release）…");
    execFileSync("cargo", ["build", "--release", "--manifest-path", "backend/Cargo.toml"], {
      cwd: ROOT,
      stdio: "inherit",
    });
  }

  const builtBin = path.join(
    ROOT,
    "backend",
    "target",
    "release",
    isWindows ? "eyecare-sidecar.exe" : "eyecare-sidecar"
  );
  if (!fs.existsSync(builtBin)) {
    throw new Error(`找不到构建产物：${builtBin}\n先跑一次 cargo build --release，或去掉 --skip-build`);
  }

  rmrf(STAGE);
  fs.mkdirSync(STAGE, { recursive: true });

  // 1. 按 dbx-plugin.toml 的 include 收集：manifest / ui / bin
  copyTree(path.join(ROOT, "ui"), path.join(STAGE, "ui"));
  fs.copyFileSync(path.join(ROOT, "manifest.json"), path.join(STAGE, "manifest.json"));

  const binDir = path.join(STAGE, "bin", TARGET);
  fs.mkdirSync(binDir, { recursive: true });
  fs.copyFileSync(builtBin, path.join(binDir, BIN_NAME));

  // 2. 重写 manifest 的 executable，指向包内真实路径
  const stagedManifestPath = path.join(STAGE, "manifest.json");
  const staged = JSON.parse(fs.readFileSync(stagedManifestPath, "utf8"));
  staged.entrypoints.backend.executable = `bin/${TARGET}/${BIN_NAME}`;
  fs.writeFileSync(stagedManifestPath, JSON.stringify(staged, null, 2) + "\n");

  // 3. 算哈希（此时还没有 checksums.json 自己）
  const files = walk(STAGE);
  const checksums = { algorithm: "sha256", files: {} };
  for (const rel of files.sort()) {
    checksums.files[rel] = sha256(path.join(STAGE, rel));
  }
  fs.writeFileSync(
    path.join(STAGE, "checksums.json"),
    JSON.stringify(checksums, null, 2) + "\n"
  );

  // 4. 压包，bin 下的文件给执行位
  const all = walk(STAGE).sort();
  const entries = all.map((rel) => ({
    name: rel,
    path: path.join(STAGE, rel),
    executable: rel.startsWith("bin/"),
  }));

  fs.mkdirSync(DIST, { recursive: true });
  const pkgName = `dbx-eyecare-${manifest.version}-${TARGET}.zip`;
  const pkgPath = path.join(DIST, pkgName);
  writeZip(entries, pkgPath);

  // 5. 自检：重新打开 zip，确认文件集合与 checksums 声明完全一致
  const listed = listZip(pkgPath);
  const inZip = listed.map((e) => e.name).sort();
  const declared = [...Object.keys(checksums.files), "checksums.json"].sort();

  const missing = declared.filter((n) => !inZip.includes(n));
  const extra = inZip.filter((n) => !declared.includes(n));
  if (missing.length || extra.length) {
    throw new Error(
      `覆盖检查失败\n  缺少：${missing.join(", ") || "无"}\n  多余：${extra.join(", ") || "无"}`
    );
  }

  const binEntry = listed.find((e) => e.name.startsWith("bin/"));
  if (!binEntry || (binEntry.mode & 0o111) === 0) {
    throw new Error(`侧车没有执行位：${binEntry ? binEntry.mode.toString(8) : "未找到"}`);
  }

  if (!staged.entrypoints.backend.executable || !inZip.includes(staged.entrypoints.backend.executable)) {
    throw new Error("manifest 的 executable 指向了包内不存在的文件");
  }

  const stat = fs.statSync(pkgPath);
  const artifact = {
    id: staged.id,
    version: staged.version,
    target: TARGET,
    file: pkgName,
    size: stat.size,
    sha256: sha256(pkgPath),
  };
  fs.writeFileSync(pkgPath.replace(/\.zip$/, ".artifact.json"), JSON.stringify(artifact, null, 2) + "\n");

  console.log("\n打包完成");
  console.log(`  文件    ${pkgName}`);
  console.log(`  大小    ${(stat.size / 1024 / 1024).toFixed(2)} MB`);
  console.log(`  条目    ${inZip.length} 个，checksums 精确覆盖`);
  console.log(`  侧车    ${staged.entrypoints.backend.executable}（权限 ${(binEntry.mode & 0o777).toString(8)}）`);
}

main();

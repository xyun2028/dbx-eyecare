// Eye眼 DBX 插件打包。
//
// 真正打包的是官方 CLI `dbx-plugin package`，这里只做三件事：
//   1. 保证 backend/Cargo.lock 里有 CLI 注入 patch 所需的记录（否则 --locked 会失败）
//   2. 调用 CLI 出 .dbxp，再收进 dist/
//   3. 生成上架要用的 release-candidates.json / candidates/ / publishers/
//
// 为什么不用 CLI 默认输出目录：在 Windows + MinGW 工具链下，构建临时目录若落在
// 工程内（深层路径），链接器会找不到 build script 的目标文件。换到用户目录下的
// 短路径就正常。官方模板同样如此，属于环境问题，不是本项目特有。

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { execFileSync } = require("child_process");

const ROOT = path.resolve(__dirname, "..");
const DIST = path.join(ROOT, "dist");
const TMP = path.join(os.homedir(), "dbxpack-tmp");
const REPO = "https://github.com/xyun2028/dbx-eyecare";

const args = process.argv.slice(2);
const targetArg = args.indexOf("--target");
const TARGET = targetArg >= 0 ? args[targetArg + 1] : null;

const sha256 = (file) => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const rmrf = (dir) => fs.rmSync(dir, { recursive: true, force: true });
const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));

/// CLI 以 `cargo build --locked` 编译侧车，并在命令行注入 dbx-plugin-sdk 的 patch。
/// 我们的侧车没用这个 SDK，patch 就成了「未使用」，Cargo 会想改写锁文件而被 --locked 拒绝。
/// 锁文件里预先记下这条未使用 patch，两边就都能过。
function ensureUnusedPatch() {
  const lock = path.join(ROOT, "backend", "Cargo.lock");
  const text = fs.readFileSync(lock, "utf8");
  if (text.includes('name = "dbx-plugin-sdk"')) return;

  fs.writeFileSync(
    lock,
    text.trimEnd() + '\n\n[[patch.unused]]\nname = "dbx-plugin-sdk"\nversion = "0.1.0"\n'
  );
  console.log("已补 Cargo.lock 的未使用 patch 记录");
}

function main() {
  const manifest = readJson(path.join(ROOT, "manifest.json"));
  const storeMeta = fs.existsSync(path.join(ROOT, ".dbx-store.json"))
    ? readJson(path.join(ROOT, ".dbx-store.json"))
    : {};

  console.log(`打包 ${manifest.name} v${manifest.version}（官方 CLI）`);
  ensureUnusedPatch();

  rmrf(TMP);
  fs.mkdirSync(TMP, { recursive: true });

  const cliArgs = ["package", ".", "--output-dir", TMP];
  if (TARGET) cliArgs.push("--target", TARGET);
  execFileSync("dbx-plugin", cliArgs, { cwd: ROOT, stdio: "inherit", shell: true });

  const built = fs.readdirSync(TMP).filter((f) => f.endsWith(".dbxp"));
  if (built.length !== 1) {
    throw new Error(`预期一个 dbxp，实际 ${built.length} 个：${built.join(", ") || "无"}`);
  }

  const pkgName = built[0];
  const pkgPath = path.join(TMP, pkgName);
  const artifact = readJson(pkgPath.replace(/\.dbxp$/, ".artifact.json"));

  fs.mkdirSync(DIST, { recursive: true });
  for (const old of fs.readdirSync(DIST)) {
    const isOurs = old.startsWith(manifest.id + "-") && (old.endsWith(".dbxp") || old.endsWith(".artifact.json"));
    if (isOurs) fs.rmSync(path.join(DIST, old));
  }
  fs.copyFileSync(pkgPath, path.join(DIST, pkgName));
  fs.copyFileSync(pkgPath.replace(/\.dbxp$/, ".artifact.json"), path.join(DIST, path.basename(pkgName).replace(/\.dbxp$/, ".artifact.json")));

  const outPkg = path.join(DIST, pkgName);
  const realSha = sha256(outPkg);
  if (realSha !== artifact.sha256) {
    throw new Error(`artifact 的 sha256 与文件不符：${artifact.sha256} vs ${realSha}`);
  }

  const tag = `v${manifest.version}`;
  const candidate = {
    schemaVersion: 1,
    id: manifest.id,
    publisher: manifest.publisher,
    version: manifest.version,
    name: manifest.name,
    source: `${REPO}/tree/${tag}`,
    license: storeMeta.license || "MIT",
    targets: [
      {
        target: artifact.target,
        url: `${REPO}/releases/download/${tag}/${pkgName}`,
        sha256: artifact.sha256,
        size: artifact.size,
      },
    ],
  };

  fs.mkdirSync(path.join(DIST, "candidates"), { recursive: true });
  fs.mkdirSync(path.join(DIST, "publishers"), { recursive: true });
  fs.writeFileSync(path.join(DIST, "release-candidates.json"), JSON.stringify(candidate, null, 2) + "\n");
  fs.writeFileSync(
    path.join(DIST, "candidates", `${manifest.id}.json`),
    JSON.stringify(candidate, null, 2) + "\n"
  );
  fs.writeFileSync(
    path.join(DIST, "publishers", `${manifest.publisher}.json`),
    JSON.stringify({ id: manifest.publisher, name: manifest.publisher, status: "unverified" }, null, 2) + "\n"
  );

  rmrf(TMP);

  console.log("\n完成");
  console.log(`  安装包  dist/${pkgName}  ${(artifact.size / 1024 / 1024).toFixed(2)} MB`);
  console.log(`  目标    ${artifact.target}`);
  console.log("  签名    未签名候选包，审核通过后由 DBX Store 统一签名");
  console.log("  上架    dist/release-candidates.json");
  console.log(`          dist/candidates/${manifest.id}.json`);
  console.log(`          dist/publishers/${manifest.publisher}.json`);
}

main();

// Eye眼 DBX 插件打包。
//
// 真正打包的是官方 CLI `dbx-plugin package`，这里只做四件事：
//   1. 保证 backend/Cargo.lock 里有 CLI 注入 patch 所需的记录（否则 --locked 会失败）
//   2. 调用 CLI 出 .dbxp，再收进 dist/
//   3. 把本次 target 合并进已有的上架文件，不覆盖其它平台的产物
//   4. 打印还差哪些平台
//
// 官方 CLI 只允许在目标平台上打包（拿别的 target 会直接拒绝），
// 所以多平台不是一条命令能出全的：每个平台各跑一次，产物在 dist/ 里累积。
//
// 为什么不用 CLI 默认输出目录：在 Windows + MinGW 工具链下，构建临时目录若落在
// 工程内（深层路径），链接器会找不到 build script 的目标文件。换到用户目录下的
// 短路径就正常。官方模板同样如此，属于环境问题，不是本项目特有。

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { execFileSync } = require("child_process");

const { buildCandidate, buildRelease, mergeTarget } = require("./listing");

const ROOT = path.resolve(__dirname, "..");
const DIST = path.join(ROOT, "dist");
const TMP = path.join(os.homedir(), "dbxpack-tmp");
const REPO = "https://github.com/xyun2028/dbx-eyecare";
/// 商店的 icon 字段要一个能直接取的图片地址，不能是包内相对路径
const RAW = "https://raw.githubusercontent.com/xyun2028/dbx-eyecare";

/// 商店认的 target 名。打不全不算错，但要让人知道缺哪些。
const EXPECTED_TARGETS = [
  "windows-x64",
  "darwin-x64",
  "darwin-arm64",
  "linux-x64",
  "linux-arm64",
];

const args = process.argv.slice(2);
const targetArg = args.indexOf("--target");
const TARGET = targetArg >= 0 ? args[targetArg + 1] : null;

const sha256 = (file) => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const rmrf = (dir) => fs.rmSync(dir, { recursive: true, force: true });
const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));
const writeJson = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, 2) + "\n");

/// 改了 Cargo.toml 的版本号之后，Cargo.lock 里记的还是旧版本，
/// CLI 的 `cargo build --locked` 会直接拒绝。先在离线模式对齐一次，
/// 免得每次发版都卡在版本号这种小事上。
function syncLockFile() {
  const lock = path.join(ROOT, "backend", "Cargo.lock");
  const before = fs.readFileSync(lock, "utf8");

  try {
    execFileSync("cargo", ["metadata", "--format-version", "1", "--offline"], {
      cwd: path.join(ROOT, "backend"),
      stdio: "ignore",
    });
  } catch {
    // 离线解析失败一般意味着依赖缓存不全，让后面的打包流程报更具体的错
    return;
  }

  if (fs.readFileSync(lock, "utf8") !== before) {
    console.log("已让 Cargo.lock 跟随 Cargo.toml 的版本");
  }
}

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

/// 同一个 target 只留最新的一份；换了版本就把旧版本的包清掉。
function pruneDist(manifest, version, target) {
  for (const name of fs.readdirSync(DIST)) {
    if (!name.startsWith(manifest.id + "-")) continue;
    if (!name.endsWith(".dbxp") && !name.endsWith(".artifact.json")) continue;

    const sameVersion = name.includes(`-${version}-`);
    const sameTarget = name.includes(`-${version}-${target}.`);
    if (sameTarget || !sameVersion) fs.rmSync(path.join(DIST, name));
  }
}

function main() {
  const manifest = readJson(path.join(ROOT, "manifest.json"));
  const storeMeta = fs.existsSync(path.join(ROOT, ".dbx-store.json"))
    ? readJson(path.join(ROOT, ".dbx-store.json"))
    : {};

  console.log(`打包 ${manifest.name} v${manifest.version}（官方 CLI）`);
  syncLockFile();
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

  // 用 CLI 回写的 target 做准，而不是命令行参数：不传参数时它取当前主机
  const target = artifact.target;
  const version = manifest.version;
  const tag = `v${version}`;
  const url = `${REPO}/releases/download/${tag}/${pkgName}`;

  fs.mkdirSync(DIST, { recursive: true });
  pruneDist(manifest, version, target);
  fs.copyFileSync(pkgPath, path.join(DIST, pkgName));
  fs.copyFileSync(
    pkgPath.replace(/\.dbxp$/, ".artifact.json"),
    path.join(DIST, pkgName.replace(/\.dbxp$/, ".artifact.json"))
  );

  const outPkg = path.join(DIST, pkgName);
  const realSha = sha256(outPkg);
  if (realSha !== artifact.sha256) {
    throw new Error(`artifact 的 sha256 与文件不符：${artifact.sha256} vs ${realSha}`);
  }

  // ---- 上架文件 ----
  // 两份格式不同，都不能省：
  //   release-candidates.json 挂在 Release 上，给商店的自动同步流程读
  //   candidates/<id>.json 是提 PR 用的候选，按 CONTRIBUTING 的字段来

  const relPath = path.join(DIST, "release-candidates.json");
  const existingRel = fs.existsSync(relPath) ? readJson(relPath) : null;
  const sameRelease =
    existingRel &&
    existingRel.plugin?.id === manifest.id &&
    existingRel.plugin?.version === version;

  const artifacts = mergeTarget(sameRelease ? existingRel.artifacts : [], {
    target,
    url,
    sha256: artifact.sha256,
    size: artifact.size,
  });

  writeJson(relPath, buildRelease({ manifest, store: storeMeta, artifacts }));

  const candidatePath = path.join(DIST, "candidates", `${manifest.id}.json`);
  const existingCandidate = fs.existsSync(candidatePath) ? readJson(candidatePath) : null;
  const sameCandidate =
    existingCandidate &&
    existingCandidate.id === manifest.id &&
    existingCandidate.version === version;

  const targets = mergeTarget(sameCandidate ? existingCandidate.targets : [], {
    target,
    url,
    sha256: artifact.sha256,
    size: artifact.size,
  });

  const candidate = buildCandidate({
    manifest,
    store: storeMeta,
    artifacts: targets,
    repo: REPO,
    rawBase: RAW,
    tag,
  });

  fs.mkdirSync(path.join(DIST, "candidates"), { recursive: true });
  fs.mkdirSync(path.join(DIST, "publishers"), { recursive: true });
  writeJson(candidatePath, candidate);
  writeJson(path.join(DIST, "publishers", `${manifest.publisher}.json`), {
    id: manifest.publisher,
    name: manifest.publisher,
    status: "unverified",
  });

  rmrf(TMP);

  const missing = EXPECTED_TARGETS.filter((t) => !targets.some((item) => item.target === t));

  console.log("\n完成");
  console.log(`  安装包  dist/${pkgName}  ${(artifact.size / 1024 / 1024).toFixed(2)} MB`);
  console.log(`  目标    ${target}`);
  console.log("  签名    未签名候选包，审核通过后由 DBX Store 统一签名");
  console.log(`  已覆盖  ${targets.map((item) => item.target).join(", ")}`);
  if (missing.length) {
    console.log(`  还缺    ${missing.join(", ")}`);
    console.log("          官方 CLI 不允许跨平台打包，需要在对应平台上各跑一次");
  }
  console.log("  上架    dist/release-candidates.json");
  console.log(`          dist/candidates/${manifest.id}.json`);
  console.log(`          dist/publishers/${manifest.publisher}.json`);
}

main();

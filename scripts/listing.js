// 上架文件的装配逻辑。
//
// 两处会用到：
//   scripts/package.js         单机打包后，把本次 target 并进已有列表
//   scripts/merge-artifacts.js CI 里各平台产物都下来之后，一次性合成完整列表
//
// 放一起是因为这两种格式很容易写歪：release-candidates.json 是挂 Release 给
// 同步流程读的（plugin + artifacts），candidates/<id>.json 是提 PR 用的
// （schemaVersion + targets）。字段来源也不同，掺在一起就容易出现
// 「description 用了中文、本地化却写 en」这种方向反掉的问题。

function sortTargets(list) {
  return [...list].sort((a, b) => a.target.localeCompare(b.target));
}

/// 同一个 target 只留一份，其余原样保留。
function mergeTarget(list, entry) {
  const kept = (list || []).filter((item) => item.target !== entry.target);
  return sortTargets([...kept, entry]);
}

/// 挂 Release 的聚合文件：商店的同步流程读这一份。
function buildRelease({ manifest, store, artifacts }) {
  return {
    plugin: {
      id: manifest.id,
      publisher: manifest.publisher,
      version: manifest.version,
      name: manifest.name,
      // 商店的展示语言是「英文作底 + zh-CN 本地化」，
      // 已收录的插件都是这个方向，别反过来
      description: store.description || manifest.description,
      permissions: manifest.permissions,
    },
    artifacts: sortTargets(artifacts),
  };
}

/// 提 PR 用的候选文件：字段以 dbx-store 的 plugin-candidate schema 为准。
function buildCandidate({ manifest, store, artifacts, repo, rawBase, tag }) {
  return {
    schemaVersion: 1,
    id: manifest.id,
    publisher: manifest.publisher,
    version: manifest.version,
    name: manifest.name,
    description: store.description || manifest.description,
    icon: `${rawBase}/${tag}/${manifest.icon}`,
    tags: store.tags || [],
    permissions: manifest.permissions,
    source: `${repo}/tree/${tag}`,
    homepage: store.homepage || manifest.homepage,
    license: store.license || "MIT",
    releaseNotes: store.releaseNotes || "",
    localizations: store.localizations || {},
    targets: sortTargets(artifacts),
  };
}

module.exports = { buildCandidate, buildRelease, mergeTarget, sortTargets };

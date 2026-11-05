# Eye眼 · DBX 插件

在 DBX 里直接调屏幕色温，按时段自动切换，每 20 分钟提醒你抬头看远处。

桌面版 [eyecare](https://github.com/xyun2028/eyecare) 的引擎被搬进了 DBX：
调的是显示器的 Gamma 查找表，不是在屏幕上盖一层半透明色板——画面不会发灰，
多块屏幕还能各调各的。

> 界面截图等插件在真实宿主里跑通后再补，这里先不放渲染图。

## 功能

- **色温**：1000–6500K 连续调节，配亮度
- **预设**：原色 / 轻度 / 办公 / 夜间 / 睡前 / 豆沙绿 / 纸质暖黄
- **按时间自动**：白天与夜间两档色温 + 两个切换时刻，切换前后线性过渡
- **20-20-20**：每工作 20 分钟提醒一次，休息 20 秒；倒计时就在面板上
- **逐屏独立**：多显示器可以只调其中一块

## 为什么是 Gamma，不是遮罩

常见做法是在窗口上铺一层半透明色板，代价是画面整体对比度被压平，
黑色变成深灰，而且独占全屏的内容会绕过它。

改 Gamma 表是直接改显卡输出的颜色映射曲线，真正改变发色，几乎不占显卡。
代价是：开了 HDR 会失效，而且各平台接口完全不同。

## 结构

DBX 把插件界面跑在 `sandbox="allow-scripts"` 的 iframe 里，没有网络、没有磁盘、
不能起进程。所以凡是碰系统的活都必须放在侧车里：

```
ui/                 沙箱前端：只管显示和发 RPC
backend/            Rust 侧车：stdin/stdout 上行分隔 JSON-RPC 2.0
  src/main.rs       读帧、分发、退出时还原屏幕
  src/rpc.rs        方法表与握手
  src/gamma.rs      逐屏 Gamma 读写、HDR 查询
  src/color.rs      色温 → Gamma 表
  src/model.rs      预设、时段规则、休息计时
  src/state.rs      持久化与「把配置写到屏幕上」
manifest.json       插件清单与贡献点
dbx-plugin.toml     打包配置
```

前端调用只有一条路：

```js
const state = await window.dbxPlugin.invoke("eyecare/state", {});
```

## RPC 方法

| 方法 | 说明 |
|---|---|
| `eyecare/state` | 全量状态：配置、预设、显示器、当前生效的色调、休息倒计时 |
| `eyecare/config` | 改配置并立即应用（`enabled` / `preset` / `kelvin` / `brightness` / `target` / `schedule`） |
| `eyecare/preset` | 选一个预设 |
| `eyecare/tint` | 自定义色温或亮度，自动切到 custom |
| `eyecare/break` | 休息计时：开关、间隔、`action: "skip"` |

侧车推给前端的事件只有一条：

```
evt/break.due   { restSeconds, at }
```

## 构建与打包

```bash
node scripts/package.js                    # 构建 release 并打成 .dbxp
node scripts/package.js --skip-build       # 复用已有的 release 产物
node scripts/package.js --target linux-x64 # 指定目标平台
```

产物是 `.dbxp`——本质是个 zip 容器，换扩展名而已，宿主按这个名字认。
文件名用插件 id 而不是显示名：`<id>-<version>-<target>.dbxp`。

脚本做四件事：

1. 把 `manifest.json` 的 `backend.executable` 重写成包内真实路径（`bin/<target>/<binary>`）
2. 生成 `checksums.json`，用 sha256 精确覆盖除自身外的所有文件
3. 按 Unix 权限位压包：`bin/` 下 755，其余 644，`version-made-by` 标为 Unix
4. **重新打开包自检**：文件集合是否与 checksums 完全一致、侧车有没有执行位、
   manifest 指向的文件是否真的存在

自检不过就直接非零退出。这三条是宿主安装器的硬约定，不满足只会得到一句
incompatible，不会有任何解释，所以宁可在本地拦住。

产物在 `dist/`：

```
com.eyecare.dbx-0.1.5-windows-x64.dbxp              # 安装包
com.eyecare.dbx-0.1.5-windows-x64.artifact.json     # target / url / sha256 / size
release-candidates.json                             # 挂到 Release，供商店同步流程读取
candidates/com.eyecare.dbx.json                     # 提给 dbx-store 的候选文件
publishers/xyun2028.json                            # 提给 dbx-store 的发布者记录
```

测试：

```bash
cargo test --manifest-path backend/Cargo.toml
```

## 发布到官方商店

商店目录在 [t8y2/dbx-store](https://github.com/t8y2/dbx-store)（不是 `t8y2/dbx`）。

1. 把 `dist/` 里的 dbxp、artifact 和 `release-candidates.json` 挂到 GitHub Release，
   Tag 用 `v<version>`。**dbxp 不要提交进 Git 历史。**
2. Fork `dbx-store`，向 `main` 提一个 PR，放入 `publishers/xyun2028.json` 和
   `candidates/com.eyecare.dbx.json`（这两个文件 `dist/` 里已经生成好了）。
   PR 里不要放 dbxp 二进制、源码副本或任何私钥。
3. 维护者审核源码、清单、权限、SHA-256 与原生行为，通过后由 DBX Store 统一加
   Ed25519 签名。**作者不需要自己签名，也不要填 `signingKeyId`。**
4. 签名结果回写到同一个 PR，合并后进入商店目录。

之后发版只需递增 `manifest.json` 的 `version`、发新 Release，不必再改商店文件。
已发布的 id + 版本组合不可覆盖，任何改动都要升版本重新审核。

商店文案（名称、简介、标签、英文说明）在 `.dbx-store.json`，只在首次上架或主动
修改商店信息时更新。

## 数据落在哪

宿主通过 `DBX_PLUGIN_DATA_DIR` 指定目录，插件在里面放两个文件：

- `config.json` —— 配置
- `original_ramp.txt` —— 改屏之前拍下的原始 Gamma 表

第二个是关键。插件被强杀时来不及做任何清理，下次启动发现它还在，
就先把屏幕还原再继续。没有它，一次崩溃就会让屏幕一直偏色。

## 已验证 / 未验证

**验证过的：**

- 侧车握手：喂 `plugin/initialize` 进去，返回 `protocolVersion` / `capabilities` / `plugin.id+version`
- `eyecare/state`：读出两块 1920×1080 显示器、HDR 状态、7 个预设
- 色温与滤色的换算：8 个单元测试（曲线单调、低色温压蓝、滤色按比例）
- 退出时还原屏幕：stdin 关闭后把原始表写回

**没验证的：**

- **没有在真实 DBX 宿主里加载过**。宿主侧的事件形状、`manifest.json` 的字段、
  侧车首帧格式以实际表现为准，这里按 DBX 插件规范实现，可能有出入。
- 打包脚本只在 Windows 上跑过；`--target linux-x64` 会照常出包，但 Linux 侧的侧车还是空壳
- 没有签名。未签名包需要在插件中心打开「允许安装未签名开发包」才能装
- HDR 开启时 Gamma 表会被系统绕过，界面上暂时只是不生效，没有提示
- 只有 Windows 实现；macOS / Linux 的后端是空壳，编译能过但没有功能

## 已知限制

- 亮度是软件层缩放，不是调显示器背光，最低压到 10%
- 独占全屏的程序不受影响（Gamma 表被绕过，属正常现象）
- 预设只能选内置的七个，还不能自己存
- 退出 DBX 后色温会被还原——插件没有常驻进程

## 许可证

[MIT](LICENSE)

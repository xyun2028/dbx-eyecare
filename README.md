# Eye眼 · DBX 插件

在 DBX 里直接调屏幕色温，按时段自动切换，每 20 分钟提醒你抬头看远处。

桌面版 [eyecare](https://github.com/xyun2028/eyecare) 的引擎被搬进了 DBX：
调的是显示器的 Gamma 查找表，不是在屏幕上盖一层半透明色板——画面不会发灰，
多块屏幕还能各调各的。

## 平台

| 平台 | 怎么改屏幕 | 状态 |
|---|---|---|
| Windows x64 | GDI `SetDeviceGammaRamp`，HDR 用 DisplayConfig 查 | 实机跑过 |
| Windows ARM64 | 同一套 Win32 调用，没有架构相关的代码 | 只过了编译 |
| macOS Intel / Apple Silicon | CoreGraphics `CGSetDisplayTransferByTable` | 只过了编译，没在真机跑过 |
| Linux X11（x64 / arm64） | RandR `SetCrtcGamma`，纯 Rust 的 x11rb | 只过了编译，没在真机跑过 |
| Linux Wayland | 没有公开接口 | 明确提示不支持 |
| 其它 Unix | 空实现 | 明确提示不支持 |

Wayland 这条要说清楚：`wlr-gamma-control` 只有 wlroots 系合成器支持，
GNOME 和 KDE 都不让第三方改 Gamma 表。XWayland 能连上 X，但改的是它自己那份
假表，桌面本身不变。所以插件在 Wayland 会话里会直接把原因写在面板上，
而不是让开关看起来生效、实际什么都没发生。

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
  src/state.rs      持久化、备份、把配置写到屏幕上
  src/color.rs      色温 → Gamma 表
  src/model.rs      预设、时段规则、休息计时
  src/gamma/
    mod.rs          公共的 Ramp 类型、重采样、平台分派
    windows.rs      GDI + DisplayConfig
    macos.rs        CoreGraphics
    linux.rs        X11 / RandR
    unsupported.rs  其它平台
manifest.json       插件清单与贡献点
dbx-plugin.toml     打包配置
```

三家的 Gamma 表长得不一样，内部先统一成一条表：三个通道等长、每个点
0..=65535。Windows 固定 256 个点，X11 的长度由 crtc 决定（常见 256，也可能
1024），macOS 用 0.0–1.0 的 float。色温算法只算 256 个点，写屏时再按目标平台
重采样，所以算法本身不用知道底下是什么系统。

前端调用只有一条路：

```js
const state = await window.dbxPlugin.invoke("eyecare/state", {});
```

## RPC 方法

| 方法 | 说明 |
|---|---|
| `eyecare/state` | 全量状态：配置、预设、显示器、当前生效的色调、休息倒计时、平台能力 |
| `eyecare/config` | 改配置并立即应用（`enabled` / `preset` / `kelvin` / `brightness` / `target` / `schedule`） |
| `eyecare/preset` | 选一个预设 |
| `eyecare/tint` | 自定义色温或亮度，自动切到 custom |
| `eyecare/break` | 休息计时：开关、间隔、`action: "skip"` |

`eyecare/state` 里有两个字段是给平台差异用的：

```json
{
  "platform": "linux",
  "arch": "aarch64",
  "support": { "available": false, "session": "wayland", "reason": "…" }
}
```

`support.available` 为 false 时，界面上会直接把 `reason` 显示出来，
不用用户猜为什么拖滑杆没反应。

侧车推给前端的事件只有一条：

```
evt/break.due   { restSeconds, at }
```

## 构建与打包

打包用的是官方 CLI，先装一次：

```bash
npm install --global @dbx-app/plugin-cli
```

然后：

```bash
node scripts/package.js
```

**官方 CLI 不允许跨平台打包。** 在 Windows 上拿 `--target darwin-arm64` 会被直接拒绝，
报 `does not match build host`。所以多平台只能每个平台各跑一次，产物在 `dist/` 里累积。

脚本做四件事：

1. 保证 `backend/Cargo.lock` 里有 CLI 注入 `dbx-plugin-sdk` patch 所需的记录
   （我们的侧车没用这个 SDK，patch 属于「未使用」，不记下来 `--locked` 会直接失败）
2. 调 `dbx-plugin package` 出 `.dbxp` 和 artifact，收进 `dist/`
3. 把本次 target 合并进 `release-candidates.json` 和 `candidates/<id>.json`，
   不覆盖其它平台已经打好的产物
4. 打印还缺哪些平台

仓库里带了一份 GitHub Actions 工作流（`.github/workflows/build.yml`），
矩阵覆盖 Windows x64、macOS x64/arm64、Linux x64/arm64，各平台原生构建并跑单测。
它只上传产物，不自动发 Release。

`.dbxp` 本质是 zip 容器，宿主认的是文件名：`<plugin-id>-<version>-<target>.dbxp`。
清单里的 `backend.executable` 指向 `bin/<target>/<binary>`，由 CLI 按目标平台写入。

> **Windows 上的坑**：CLI 默认把构建目录放在输出目录里。如果输出目录在工程内（路径较深），
> MinGW 链接器会找不到 build script 的目标文件。所以脚本改用 `~/dbxpack-tmp` 作为临时输出，
> 出包后再挪进 `dist/`。官方模板在同样的环境里也会失败，属于工具链问题。

产物在 `dist/`：

```
com.eyecare.dbx-0.2.0-windows-x64.dbxp              # 安装包
com.eyecare.dbx-0.2.0-windows-x64.artifact.json     # target / url / sha256 / size
release-candidates.json                             # 挂到 Release，供商店同步流程读取
candidates/com.eyecare.dbx.json                     # 提给 dbx-store 的候选文件
publishers/xyun2028.json                            # 提给 dbx-store 的发布者记录
```

不同平台的包文件名不同，会并排放在 `dist/` 里；两份上架文件里的 `artifacts` / `targets`
数组也会合并，所以一台机器打一次就能凑齐一份完整的多平台候选。

测试（不涉及打包，普通 cargo 即可）：

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

文件首行是版本标记：

- `eyecare-dbx-v2` 后面是 JSON，每条记录带 `id`、三个通道和表长。改这个格式是因为
  X11 的表长不固定，老的定长 768 存不下。
- `eyecare-dbx-v1` 是旧的 CSV 格式，仍然能读。升级到多平台版本后，上一版留下的备份
  不会失效，屏幕不会卡在偏色状态。

## 已验证 / 未验证

**验证过的：**

- Windows 实机：握手返回 `protocolVersion` / `capabilities` / `plugin.id+version`
- Windows 实机：`eyecare/state` 读出两块 1920×1080 显示器、HDR 状态、7 个预设
- Windows 实机：写表返回 `ok=true`，退出后屏幕还原；关掉开关写回线性表
- 色温与滤色的换算、表重采样、备份往返：21 个单元测试
- 编译：`x86_64-pc-windows-gnu`（宿主）、`aarch64-pc-windows-msvc`、
  `x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`、
  `x86_64-apple-darwin`、`aarch64-apple-darwin` 全部 `cargo check` 通过
- 打包：Windows 出包，包内 manifest 的 `backend.executable` 指向 `bin/windows-x64/`

**没验证的：**

- **macOS 和 Linux 没有在真机上跑过。** 这两份实现只保证编译通过、API 用法正确，
  实际的 gamma 写入效果、多屏排序、权限行为都要等真机验证。
- **ARM 硬件没跑过**，只做了交叉编译检查。
- **没有在真实 DBX 宿主里加载过**。宿主侧的事件形状、`manifest.json` 的字段、
  侧车首帧格式以实际表现为准。
- Wayland 的降级提示写了，但没有在 Wayland 会话里实际触发过。
- macOS 的 EDR/HDR 状态查不到，`hdr` 字段返回 `null`，界面上不会提示。
- 没有签名。未签名包需要在插件中心打开「允许安装未签名开发包」才能装。

## 已知限制

- 亮度是软件层缩放，不是调显示器背光，最低压到 10%
- 独占全屏的程序不受影响（Gamma 表被绕过，属正常现象）
- HDR 开启时 Gamma 表会被系统绕过，界面上会提示（仅 Windows 能查到 HDR 状态）
- Wayland 会话不支持，会在面板上说明原因
- macOS 上如果系统开了原彩显示（True Tone）或 Night Shift，会和插件叠加，
  最终效果不是插件单独算出来的那个
- 预设只能选内置的七个，还不能自己存
- 退出 DBX 后色温会被还原——插件没有常驻进程

## 许可证

[MIT](LICENSE)

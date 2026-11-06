# Eye眼 · DBX 插件

在 DBX 里直接调屏幕色温，按一天的作息自动变，每 20 分钟提醒你抬头看远处。

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
- **预设**：七个内置，加上自己存的任意组合——名字、色温（或滤色）、亮度一起记下来
- **24 小时曲线**：一天里放多个锚点，每个锚点定一个色温。锚点之间保持不动，
  临近下一个锚点才按设定的分钟数过渡，屏幕不会一整天都在缓慢变色
- **日出日落**：填经纬度，按当地太阳时间自动决定什么时候变暖，不用每季度手动改时刻
- **工作日 / 周末**：两套曲线。周末可以单独设，也可以跟着工作日走
- **20-20-20**：每工作 20 分钟提醒一次，休息 20 秒。到点屏幕会短暂变暖压暗，
  默认 6 秒后恢复（2–60 秒可配），不打开面板也能察觉；也可以关掉这个行为，
  只保留面板里的横幅
- **用眼记录**：今天亮了多久、休息了几次、最长连续多久，存最近 30 天，
  并按钟点累计，供「用眼建议」分析
- **逐屏独立**：每块屏各自决定要不要参与，以及跟随全局、用自己的色温、还是用自己的
  滤色；亮度和色温/颜色都能单独设。也可在「应用到」里快速只选一块屏
- **锚点增删**：曲线上的锚点可以任意加减，最多 12 个，最少留 1 个
- **在场感知**：超过两分钟没碰键鼠就暂停提醒和用眼计时，回来接着算；
  休息期间若仍在用电脑，那一次不计入「休息了几次」
- **全屏让位 / 演示模式**：前台是全屏内容时暂停写屏与提醒；也可一键暂停 60 分钟，
  适合投屏和开会，到点自动恢复
- **抢桌检测**：写屏前读回 Gamma 表，和上次自己写的比对。发现被别的软件改过
  （f.lux、Night Shift、显卡驱动）就暂停写屏并在面板说明，不互相覆盖
- **用眼建议**（默认关）：分析本地记录里最容易连续用眼的时段，给一句结论和
  一个「应用」按钮。开关不打开不分析，不点应用不改配置
- **转暖倒数**：面板显示距离下一次转暖还有多久，日出日落模式下跟着当天日落走
- **内置使用说明**：首页右上角「使用说明」按钮打开 11 章弹框，覆盖调色、预设、曲线、
  逐屏、20-20-20、用眼建议、隐私与常见问题。整篇内联在页面里，离线可读，
  不请求任何外部资源

## 为什么是 Gamma，不是遮罩

常见做法是在窗口上铺一层半透明色板，代价是画面整体对比度被压平，
黑色变成深灰，而且独占全屏的内容会绕过它。

改 Gamma 表是直接改显卡输出的颜色映射曲线，真正改变发色，几乎不占显卡。
代价是：开了 HDR 会失效，而且各平台接口完全不同。

## 结构

DBX 把插件界面跑在 `sandbox="allow-scripts"` 的 iframe 里，没有网络、没有磁盘、
不能起进程。所以凡是碰系统的活都必须放在侧车里：

```
ui/                 沙箱前端：只管显示和发 RPC；使用说明整篇内联在 index.html
backend/            Rust 侧车：stdin/stdout 上行分隔 JSON-RPC 2.0
  src/main.rs       读帧、分发、退出时还原屏幕
  src/rpc.rs        方法表与握手
  src/state.rs      持久化、备份、把配置写到屏幕上、休息提示
  src/color.rs      色温 → Gamma 表
  src/model.rs      预设、24 小时曲线、日出日落规则、休息计时
  src/sun.rs        本地算日出日落（NOAA 公式，不联网）
  src/stats.rs      用眼记录（总量 + 24 个钟点槽）
  src/presence/     在场感知：距上次键鼠输入多久、前台是否全屏（按平台分派）
  src/insight.rs    用眼建议：只生成建议，由界面显式「应用」才写进曲线
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
| `eyecare/state` | 全量状态：配置、预设、显示器、当前生效的色调、曲线、日出日落时刻、休息倒计时、用眼统计、平台能力 |
| `eyecare/config` | 改配置并立即应用（`enabled` / `preset` / `kelvin` / `brightness` / `target` / `schedule`） |
| `eyecare/preset` | 选一个预设，内置和自定义都走这里 |
| `eyecare/preset.save` | 把当前的色温（或滤色）和亮度存成自定义预设，可带 `name` |
| `eyecare/preset.rename` | 改自定义预设的名字 |
| `eyecare/preset.delete` | 删一个自定义预设 |
| `eyecare/tint` | 自定义色温或亮度，自动切到 custom |
| `eyecare/break` | 休息计时：开关、间隔、`restSeconds`、`dimScreen`、`dimSeconds`（压暗持续几秒）、`action: "skip"` / `"endRest"` |
| `eyecare/screen` | 改单块屏：`id` / `enabled` / `mode: "follow"｜"custom"｜"filter"` / `kelvin` / `color`（RGB 数组或 null）/ `brightness` |
| `eyecare/presence` | `idleAware` / `fullscreenPause` / `realRest` / `presentationMinutes`（0 表示结束演示） |
| `eyecare/insight` | `enabled` 开关；`action: "apply"` 只把建议锚点写进曲线，不替用户打开自动 |
| `eyecare/gamma.resume` | 抢桌提示后重新接管屏幕 |

`eyecare/state` 里除了上表这些配置，还多出四组只读字段：

```json
{
  "screens": [
    { "id": "\\\\.\\DISPLAY1", "name": "DISPLAY1", "enabled": true,
      "mode": "filter", "kelvin": 4000, "color": [199, 237, 204], "brightness": 0.9,
      "effectiveKelvin": null, "effectiveRgb": [199, 237, 204],
      "effectiveKind": "rgb", "hdr": false }
  ],
  "presence": { "idleSeconds": 7, "idleSupported": true, "fullscreen": false,
    "away": false, "pausedReason": "", "presentationLeftSeconds": 0,
    "note": "只读取距上次键鼠输入的秒数，不记录按键内容、不读窗口标题、不联网。" },
  "insight": { "enabled": false, "summary": "…", "at": 1290,
    "kelvin": 3500, "canApply": false, "days": 5 },
  "gammaConflict": null
}
```

`gammaConflict` 有值时表示那块屏的 Gamma 表被别的程序改过，插件已暂停写屏，
要等界面点「重新接管」。`idleSupported` 为 false 时（Wayland、无图形会话），
空闲暂停自动退回旧的固定计时，不会静悄悄停掉提醒。

`presence` 用到的系统接口按平台分派，和 gamma 层同构：
Windows 是 `GetLastInputInfo` + `GetForegroundWindow`/`GetWindowRect`；
macOS 是 `CGEventSourceSecondsSinceLastEventType`（全屏判定暂未实现）；
Linux/X11 是 screensaver 扩展 + EWMH `_NET_WM_STATE_FULLSCREEN`。
全部只做查询，不订阅输入、不读窗口标题、不联网、不起子进程。

`schedule` 是这轮改动最大的地方：

```json
{
  "enabled": true,
  "mode": "clock",
  "points": [{ "at": 420, "kelvin": 5500 }],
  "weekendPoints": [],
  "transition": 60,
  "lat": 22.55,
  "lon": 114.06,
  "sunMargin": 40,
  "effectivePoints": [],
  "sunTimes": { "sunrise": 402.1, "sunset": 1083.4 }
}
```

`at` 是从零点算起的分钟数。`mode` 为 `sun` 时忽略 `points`，按经纬度算出的日出日落
生成四个锚点；`weekendPoints` 为空表示周末跟工作日一样。`effectivePoints` 和
`sunTimes` 是只读的，前者是今天实际生效的曲线（日出模式下就是算出来的那四个点），
后者是当地日出日落时刻，极昼极夜时为 `null`。

老版本只有 `dayKelvin` / `nightKelvin` / `dayStart` / `nightStart` 四个字段。
读到没有 `points` 的配置时会自动补成两个锚点，所以升级不会丢设置。

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

侧车推给前端的事件有两条：

```
evt/break.due       { restSeconds, dimSeconds, dimScreen, at }
evt/gamma.conflict  { display, at }
```

`dimSeconds` 是屏幕压暗持续的秒数，和 `restSeconds`（20-20-20 的休息时长）解耦：
提醒横幅按 `restSeconds` 计时，屏幕只在 `dimSeconds` 内变暖变暗，然后自动恢复。
默认 6 秒，可配范围 2–60。

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
等五个平台都打完，最后一个 job 会把它们合成一份完整的上架材料（artifact 名 `listing`），
下载下来就是可以直接提 PR 的 `candidates/<id>.json` 和挂 Release 的
`release-candidates.json`。它不自动发 Release。

`.dbxp` 本质是 zip 容器，宿主认的是文件名：`<plugin-id>-<version>-<target>.dbxp`。
清单里的 `backend.executable` 指向 `bin/<target>/<binary>`，由 CLI 按目标平台写入。

> **Windows 上的坑**：CLI 默认把构建目录放在输出目录里。如果输出目录在工程内（路径较深），
> MinGW 链接器会找不到 build script 的目标文件。所以脚本改用 `~/dbxpack-tmp` 作为临时输出，
> 出包后再挪进 `dist/`。官方模板在同样的环境里也会失败，属于工具链问题。

产物在 `dist/`：

```
com.eyecare.dbx-0.2.1-windows-x64.dbxp              # 安装包
com.eyecare.dbx-0.2.1-windows-x64.artifact.json     # target / url / sha256 / size
release-candidates.json                             # 挂到 Release，供商店同步流程读取
candidates/com.eyecare.dbx.json                     # 提给 dbx-store 的候选文件
publishers/xyun2028.json                            # 提给 dbx-store 的发布者记录
```

不同平台的包文件名不同，会并排放在 `dist/` 里；两份上架文件里的 `artifacts` / `targets`
数组也会合并，所以一台机器打一次就能凑齐一份完整的多平台候选。

换机器打包时，把 `dist/candidates/` 和 `dist/release-candidates.json` 一起带过去：
脚本是在已有文件上追加，不会重建。否则在 Mac 上打出来的候选只含 darwin，会盖掉
Windows 那一条。

测试（不涉及打包，普通 cargo 即可）：

```bash
cargo test --manifest-path backend/Cargo.toml
```

出包之后自检一遍上架材料：

```bash
node scripts/check-listing.js
```

它按 dbx-store 的 candidate schema 检查字段（必填项、id/版本格式、URL 必须 https、
sha256 长度、产物大小上限），两份上架文件格式不同，所以分开校验。

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

宿主通过 `DBX_PLUGIN_DATA_DIR` 指定目录，插件在里面放四个文件：

- `config.json` —— 配置
- `original_ramp.txt` —— 改屏之前拍下的原始 Gamma 表
- `stats.json` —— 用眼记录，按天存，只保留最近 30 天；每天除了总量还有 24 个钟点槽。
  升级前存下来的天没有钟点槽，用眼建议会等攒够新数据再给结论，不会拿旧数据硬凑
- `sidecar.log` —— 运行日志，涨到 2 MB 就改名成 `sidecar.log.1`，只保留一份旧的，
  所以最多占 4 MB。侧车是长期驻留的，不设上限的话一天能写几十万行

第二个是关键。插件被强杀时来不及做任何清理，下次启动发现它还在，
就先把屏幕还原再继续。没有它，一次崩溃就会让屏幕一直偏色。

文件首行是版本标记：

- `eyecare-dbx-v2` 后面是 JSON，每条记录带 `id`、三个通道和表长。改这个格式是因为
  X11 的表长不固定，老的定长 768 存不下。
- `eyecare-dbx-v1` 是旧的 CSV 格式，仍然能读。升级到多平台版本后，上一版留下的备份
  不会失效，屏幕不会卡在偏色状态。

## 已验证 / 未验证

**验证过的（0.2.1 开发树，含本轮改动）：**

- Windows 实机：握手返回 `protocolVersion` / `capabilities` / `plugin.id+version`
- Windows 实机：`eyecare/state` 读出两块 1920×1080 显示器、HDR 状态、7 个预设
- Windows 实机：写表返回 `ok=true`；关掉开关写回线性表
- **真实侧车 JSON-RPC 冒烟测试**：两块屏分别下发配置后，日志里
  `DISPLAY1 tint=Kelvin(4000) screen=Custom`、`DISPLAY2 tint=Kelvin(6500) screen=Follow`，
  逐屏独立确实落到了两块屏上；把 DISPLAY1 切到 `filter` 并给 `[199,237,204]` 后，
  状态返回 `effectiveKind: "rgb"`、`effectiveRgb: [199,237,204]`、`effectiveKelvin: null`。
  `eyecare/screen`、`eyecare/presence`、`eyecare/insight`、`eyecare/gamma.resume`
  都返回预期状态，空显示器 id 会被拒绝
- **应用建议不代开自动**：预置 5 天记录后点应用，锚点 5→6，
  `schedule.enabled` 前后都是 `false`，面板给出「自己打开按时间自动」的提示
- **在场感知在 Windows 实机读到了值**：`idleSupported=true`
- **正常退出会还原屏幕**：`shutdown` 后日志出现两条 `restore-backup ok=true peak=65535`
  和 `stdin closed, screen restored`，`original_ramp.txt` 被清掉
- 73 个单元测试通过：色温换算、Gamma 重采样、备份往返、日出日落、曲线插值与跨零点、
  周历切换、用眼累加与钟点分布、锚点上限/单锚点曲线、逐屏色温与逐屏滤色（含回退）、
  休息结算、压暗时长与休息时长解耦、抢桌比对、用眼建议的分析与写入、
  写屏节流判定（目标没变 / 目标变了 / 用户操作 / 首次写屏）、日志轮转（超限、未超限、重复轮转）
- **写屏不再每 5 秒重设一次**：实机跑 66 秒，平台期日志里只有启动首帧那 2 行 `apply`
  （两块屏各一行）；同一条件下改动前是 13 拍 × 2 屏 = 26 行
- **用户操作不会被节流误伤**：间隔 3 秒连发两次 `eyecare/tint`，日志依次出现
  `kelvin: 6500 → 4000 → 3400`，每次都立即落屏
- **休息压暗能穿过节流**：先改色再等 5 秒触发休息，`tint=Kelvin { kelvin: 2475 } resting=true`
  仍然立刻写入；6 秒后再出现一次 `resting=false` 的恢复写屏（间隔只有 6 秒，远小于 20 秒节流）
- **关闭护眼立即还原**：写出 5000K 后隔 2 秒发 `enabled: false`，日志立刻出现
  `tint=None ok=true`，没有等到节流窗口结束
- **日志轮转**：单测覆盖「超过 2 MB 换成 .1」「没超限不动」「第二次轮转覆盖旧的 .1，
  不会越堆越多」
- 编译：`x86_64-pc-windows-gnu`（宿主）、`x86_64-unknown-linux-gnu`、
  `x86_64-apple-darwin` 本轮重新 `cargo check` 通过；ARM 与其余目标沿用之前的检查
- **界面在真实 Chrome 里跑过演示后端**：锚点 5→6→5、逐屏切独立改 3000K、
  逐屏切滤色并取色（色温输入自动收起、生效列出现色块）、用眼建议开关与应用
  （新增锚点但「按时间自动」仍关着）、两种倒数、演示模式开关、在场开关；
  浅色与深色两套主题截图检查过，没有溢出、重叠和控制台报错
- **使用说明弹框在真实 Chrome 里跑过**：11 章与 11 个目录项一一对应、目录跳转、
  关掉重开立刻回到顶部、Esc / 点遮罩 / 两个关闭按钮、点弹框内部不关闭、
  Tab 焦点锁在弹框内、关闭后焦点回到首页按钮；浅色、深色、600px 窄屏三套截图
  检查过，正文可选中复制，没有横向溢出和控制台报错
- 打包：本轮重新出了 Windows 包 `dist/com.eyecare.dbx-0.2.1-windows-x64.dbxp`
  （900964 字节，SHA-256 `0bc07d03c4151b068ec40e63c9b0ab3ec97f105323f258bcf9c6ada3b0317495`）。
  包内 `backend.executable` 指向 `bin/windows-x64/`，`manifest.json` 权限仍是
  `host.workbench` + `host.events`；解开包直接跑里面的 exe 验证过握手、
  `screens` / `presence` / `insight` / `gammaConflict` 字段、逐屏滤色模式和
  `break.dimSeconds=6`，关机收尾也会还原屏幕。包内 `ui/index.html` 含「使用说明」按钮、11 章正文和
  11 个目录项，`ui/style.css` / `ui/app.js` 含弹框样式与交互，整页没有任何外链。
  `dist/` 被 `.gitignore` 忽略，产物不进 Git 历史

**没验证的：**

- **macOS 和 Linux 没有在真机上跑过。** 这两份实现只保证编译通过、API 用法正确，
  实际的 gamma 写入效果、多屏排序、权限行为都要等真机验证。
  新增的 `CGEventSourceSecondsSinceLastEventType` 与 X11 screensaver / EWMH 查询同理，
  尤其 macOS 的全屏判定尚未实现，会一直按「不是全屏」处理。
- **抢桌检测没有制造过真实冲突**（没有同时跑 f.lux 之类的软件实测），
  只验证了表比对的阈值逻辑和「重新接管」这条路径。
- **ARM 硬件没跑过**，只做了交叉编译检查。
- **没有在真实 DBX 宿主里加载过**。宿主侧的事件形状、`manifest.json` 的字段、
  侧车首帧格式以实际表现为准；`evt/gamma.conflict` 也还没在宿主里接过。
- Wayland 的降级提示写了，但没有在 Wayland 会话里实际触发过。
- macOS 的 EDR/HDR 状态查不到，`hdr` 字段返回 `null`，界面上不会提示。
- 没有签名。未签名包需要在插件中心打开「允许安装未签名开发包」才能装。

## 已知限制

- 亮度是软件层缩放，不是调显示器背光，最低压到 10%
- 独占全屏的程序不受影响（Gamma 表被绕过，属正常现象）；插件只在这时暂停自己的写屏和提醒
- HDR 开启时 Gamma 表会被系统绕过，界面上会提示（仅 Windows 能查到 HDR 状态）。
  这类屏会跳过抢桌比对，否则会一直误报
- Wayland 会话不支持改色温，会在面板上说明原因；空闲与全屏检测查不到时自动退回固定计时
- macOS 上如果系统开了原彩显示（True Tone）或 Night Shift，会和插件叠加，
  最终效果不是插件单独算出来的那个
- 日出日落要自己填经纬度。插件没有定位权限，也不联网，只能按填进去的位置算
- **逐屏滤色没存过颜色时**先沿用全局滤色，全局也不是滤色就退回纸色，
  不会出现「切到滤色结果屏幕发黑」。三种模式下的亮度都是各屏自己的
- 用眼建议至少需要 3 天带钟点分布的新记录；老版本存下来的数据没有钟点槽，
  不会参与分析。**应用建议只加锚点，不会替用户打开「按时间自动」**——
  曲线关着时锚点不起作用，面板会提示用户自己到曲线卡片点开
- 用眼记录只在插件跑着、护眼开关打开、且人确实在电脑前时累加；关掉的那段时间不算
- 侧车正常退出时会把屏幕还原；如果宿主直接结束进程，这一步不会执行，
  屏幕停在当前颜色上，下次启动靠 `original_ramp.txt` 恢复

## 许可证

[Apache-2.0](LICENSE)。可以商用、修改、再分发，保留版权与许可声明即可。
提交 PR 即表示你的贡献按同一协议授权。

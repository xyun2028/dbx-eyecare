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

## 构建

```bash
cargo build --release --manifest-path backend/Cargo.toml

# 侧车要按平台放进包内目录，宿主不会帮你补路径，也不会补 .exe
mkdir -p bin/windows-x64
cp backend/target/release/eyecare-sidecar.exe bin/windows-x64/
```

`manifest.json` 里的 `entrypoints.backend.executable` 已经写成了 Windows 的包内路径。
换平台打包时要把这一段改掉（macOS / Linux 没有 `.exe`，目录名也要换）。
目前还没写打包脚本，这一步是手工的。

```bash
cargo test --manifest-path backend/Cargo.toml
```

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
- 打包脚本未写，`entrypoints` 的路径需要手工改
- HDR 开启时 Gamma 表会被系统绕过，界面上暂时只是不生效，没有提示
- 只有 Windows 实现；macOS / Linux 的后端是空壳，编译能过但没有功能

## 已知限制

- 亮度是软件层缩放，不是调显示器背光，最低压到 10%
- 独占全屏的程序不受影响（Gamma 表被绕过，属正常现象）
- 预设只能选内置的七个，还不能自己存
- 退出 DBX 后色温会被还原——插件没有常驻进程

## 许可证

[MIT](LICENSE)

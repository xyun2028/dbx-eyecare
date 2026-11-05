// Eye眼 · DBX 插件前端
//
// 宿主把界面跑在沙箱 iframe 里：没有网络、没有磁盘、不能起进程，
// 唯一通道是 window.dbxPlugin.invoke / onEvent。所以这里只做两件事：
// 显示状态、把用户操作转成 RPC。
//
// 没有宿主时（浏览器直接打开）会启用一个内存里的演示后端，
// 交互都能跑，页脚会标 MOCK。

const el = (id) => document.getElementById(id);
const SVG_NS = "http://www.w3.org/2000/svg";

/* 曲线的坐标系。viewBox 固定 720×132，容器按 aspect-ratio 等比缩放，
   所以拖动时用 getBoundingClientRect 换算，不需要管实际像素宽度。 */
const CW = 720;
const CH = 132;
const CTOP = 14;
const CBOT = 116;
const KMIN = 1000;
const KMAX = 6500;

const BUILTIN_PRESETS = [
  { id: "off", name: "原色", kelvin: 6500, color: null, swatch: "#d7e2f4" },
  { id: "soft", name: "轻度", kelvin: 5500, color: null, swatch: "#f5e3c8" },
  { id: "office", name: "办公", kelvin: 4500, color: null, swatch: "#ffd096" },
  { id: "night", name: "夜间", kelvin: 3400, color: null, swatch: "#ffb173" },
  { id: "sleep", name: "睡前", kelvin: 2700, color: null, swatch: "#ff8f4f" },
  { id: "green", name: "豆沙绿", kelvin: null, color: [199, 237, 204], swatch: "#a9ddb7" },
  { id: "paper", name: "纸质暖黄", kelvin: null, color: [250, 235, 210], swatch: "#f0d29f" },
];

let api = null;
let S = null;
let pushTimer = null;

/** 曲线里当前选中的锚点下标 */
let selectedAnchor = 0;
/** 正在编辑哪条曲线：weekday / weekend */
let editingDay = "weekday";
/** 拖动过程中的临时点，松手才提交，避免每帧一次 RPC */
let draftPoints = null;
let dragging = null;

let careRemain = 0;
let bannerLeft = 0;

/* ------------------------------------------------------------------ 小工具 */

const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

function fmtClock(seconds) {
  const s = Math.max(0, seconds | 0);
  const m = Math.floor(s / 60);
  return `${String(m).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

function fmtDuration(seconds) {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s} 秒`;
  const h = Math.floor(s / 3600);
  const m = Math.round((s % 3600) / 60);
  return h > 0 ? `${h} 小时 ${m} 分` : `${m} 分`;
}

function fmtTime(minutes) {
  const m = ((Math.round(minutes) % 1440) + 1440) % 1440;
  const h = Math.floor(m / 60);
  return `${String(h).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;
}

function toMinutes(text) {
  const [h, m] = text.split(":").map(Number);
  return clamp((h || 0) * 60 + (m || 0), 0, 1439);
}

function minutesNow() {
  const d = new Date();
  return d.getHours() * 60 + d.getMinutes() + d.getSeconds() / 60;
}

function rgbToHex(rgb) {
  return "#" + rgb.map((n) => Math.round(n).toString(16).padStart(2, "0")).join("");
}

function shade(hex, factor) {
  const v = hex.replace("#", "");
  const rgb = [0, 2, 4].map((i) => parseInt(v.slice(i, i + 2), 16));
  const c = rgb.map((n) => Math.round(clamp(n * factor, 0, 255)));
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}

/* ---------------------------------------------------------------- 宿主桥接 */

async function connectHost() {
  const real = window.dbxPlugin;
  if (real && typeof real.invoke === "function") {
    if (real.ready) await real.ready;
    return {
      mode: "real",
      invoke: (method, params) => real.invoke(method, params),
      onEvent: (fn) => real.onEvent?.(fn),
    };
  }
  return mockHost();
}

function mockHost() {
  const cfg = {
    enabled: true,
    preset: "office",
    kelvin: 4500,
    brightness: 1,
    color: null,
    target: "all",
    schedule: {
      enabled: false,
      mode: "clock",
      dayKelvin: 5500,
      nightKelvin: 3000,
      transition: 60,
      lat: 22.55,
      lon: 114.06,
      sunMargin: 40,
      points: [
        { at: 420, kelvin: 5500 },
        { at: 540, kelvin: 5200 },
        { at: 840, kelvin: 5000 },
        { at: 1110, kelvin: 4000 },
        { at: 1350, kelvin: 3200 },
      ],
      weekendPoints: [],
    },
    breakPlan: { enabled: true, workMinutes: 20, restSeconds: 20, dimScreen: true, dueAt: Date.now() + 20 * 60000 },
    customPresets: [],
  };

  const displays = [{ id: "mock-1", name: "演示屏幕", width: 1920, height: 1080, primary: true, hdr: false }];
  const listeners = new Set();
  let days = {};

  const presets = () => [
    ...BUILTIN_PRESETS.map((p) => ({ ...p, hint: "", custom: false })),
    ...cfg.customPresets.map((p) => ({
      id: p.id,
      name: p.name,
      hint: "自定义",
      kelvin: p.kelvin,
      color: p.color,
      swatch: p.color ? rgbToHex(p.color) : "#e8d6bd",
      custom: true,
    })),
  ];

  const effective = () => {
    if (!cfg.enabled) return null;
    if (cfg.schedule.enabled) return { kind: "kelvin", kelvin: 5200 };
    const p = presets().find((x) => x.id === cfg.preset);
    if (p?.color) return { kind: "rgb", rgb: p.color };
    if (cfg.preset === "custom" && cfg.color) return { kind: "rgb", rgb: cfg.color };
    return { kind: "kelvin", kelvin: p?.kelvin ?? cfg.kelvin };
  };

  const SUNRISE = 402;
  const SUNSET = 1083;

  // 和后端 sun_points 一样：日出前后各留 40 分钟过渡，中间保持白天色温
  const sunPoints = () => [
    { at: SUNRISE - 40, kelvin: cfg.schedule.nightKelvin },
    { at: SUNRISE + 40, kelvin: cfg.schedule.dayKelvin },
    { at: SUNSET - 40, kelvin: cfg.schedule.dayKelvin },
    { at: SUNSET + 40, kelvin: cfg.schedule.nightKelvin },
  ];

  const payload = () => ({
    enabled: cfg.enabled,
    preset: cfg.preset,
    kelvin: cfg.kelvin,
    brightness: cfg.brightness,
    color: cfg.color,
    target: cfg.target,
    presets: presets(),
    displays,
    effective: effective(),
    schedule: {
      ...cfg.schedule,
      effectivePoints: cfg.schedule.mode === "sun" ? sunPoints() : cfg.schedule.points,
      sunTimes: { sunrise: SUNRISE, sunset: SUNSET },
    },
    break: {
      enabled: cfg.breakPlan.enabled,
      workMinutes: cfg.breakPlan.workMinutes,
      restSeconds: cfg.breakPlan.restSeconds,
      dimScreen: cfg.breakPlan.dimScreen,
      remainingSeconds: cfg.breakPlan.enabled
        ? Math.max(0, Math.round((cfg.breakPlan.dueAt - Date.now()) / 1000))
        : null,
      resting: false,
    },
    stats: {
      today: days.today || { seconds: 0, breaks: 0, longestStreakSeconds: 0 },
      streakSeconds: 0,
      week: Array.from({ length: 7 }, (_, i) => {
        const d = new Date(Date.now() - (6 - i) * 86400000);
        const key = d.toISOString().slice(0, 10);
        const row = days[key] || { seconds: 0, breaks: 0 };
        return { date: key, seconds: row.seconds, breaks: row.breaks };
      }),
    },
    mock: true,
    platform: "mock",
    arch: "mock",
    support: { available: true, session: "mock", reason: null },
  });

  return {
    mode: "mock",
    onEvent: (fn) => listeners.add(fn),
    async invoke(method, params = {}) {
      if (method === "eyecare/state") return payload();

      if (method === "eyecare/config") {
        if (typeof params.enabled === "boolean") cfg.enabled = params.enabled;
        if (typeof params.preset === "string") cfg.preset = params.preset;
        if (typeof params.kelvin === "number") cfg.kelvin = params.kelvin;
        if (typeof params.brightness === "number") cfg.brightness = params.brightness;
        if (typeof params.target === "string") cfg.target = params.target;
        if (params.color !== undefined) cfg.color = params.color;
        if (params.schedule) {
          const s = params.schedule;
          if (s.enabled !== undefined) cfg.schedule.enabled = s.enabled;
          if (s.mode) cfg.schedule.mode = s.mode;
          if (s.points) cfg.schedule.points = s.points;
          if (s.weekendPoints) cfg.schedule.weekendPoints = s.weekendPoints;
          if (s.transition !== undefined) cfg.schedule.transition = s.transition;
          if (s.lat !== undefined) cfg.schedule.lat = s.lat;
          if (s.lon !== undefined) cfg.schedule.lon = s.lon;
          if (s.dayKelvin !== undefined) cfg.schedule.dayKelvin = s.dayKelvin;
          if (s.nightKelvin !== undefined) cfg.schedule.nightKelvin = s.nightKelvin;
        }
        return payload();
      }

      if (method === "eyecare/preset") {
        const p = presets().find((x) => x.id === params.id);
        if (p) {
          cfg.preset = p.id;
          cfg.enabled = true;
          if (p.kelvin) cfg.kelvin = p.kelvin;
          cfg.color = p.color || null;
        }
        return payload();
      }

      if (method === "eyecare/preset.save") {
        const id = `user:${Date.now()}`;
        cfg.customPresets.push({
          id,
          name: params.name || `${cfg.kelvin}K`,
          kelvin: cfg.color ? null : cfg.kelvin,
          color: cfg.color,
          brightness: cfg.brightness,
        });
        cfg.preset = id;
        return payload();
      }

      if (method === "eyecare/preset.delete") {
        cfg.customPresets = cfg.customPresets.filter((p) => p.id !== params.id);
        if (cfg.preset === params.id) cfg.preset = "office";
        return payload();
      }

      if (method === "eyecare/tint") {
        cfg.preset = "custom";
        cfg.enabled = true;
        if (typeof params.kelvin === "number") cfg.kelvin = params.kelvin;
        if (typeof params.brightness === "number") cfg.brightness = params.brightness;
        if (params.color !== undefined) cfg.color = params.color;
        return payload();
      }

      if (method === "eyecare/break") {
        if (typeof params.enabled === "boolean") {
          cfg.breakPlan.enabled = params.enabled;
          cfg.breakPlan.dueAt = params.enabled ? Date.now() + cfg.breakPlan.workMinutes * 60000 : 0;
        }
        if (typeof params.workMinutes === "number") {
          cfg.breakPlan.workMinutes = params.workMinutes;
          cfg.breakPlan.dueAt = Date.now() + params.workMinutes * 60000;
        }
        if (typeof params.dimScreen === "boolean") cfg.breakPlan.dimScreen = params.dimScreen;
        if (params.action === "skip") {
          cfg.breakPlan.dueAt = Date.now() + cfg.breakPlan.workMinutes * 60000;
        }
        return payload();
      }

      throw new Error(`mock 没有实现 ${method}`);
    },
  };
}

/* ------------------------------------------------------------------ 曲线计算 */

/** 和后端 model::sample 保持一致的取值规则，否则拖动时预览和实际写屏会对不上 */
function sampleKelvin(points, minutes, transition) {
  if (!points || points.length === 0) return KMAX;
  if (points.length === 1) return points[0].kelvin;

  const pts = [...points].sort((a, b) => a.at - b.at);
  const n = pts.length;
  const m = ((minutes % 1440) + 1440) % 1440;

  let idx = n - 1;
  for (let i = 0; i < n; i++) {
    if (pts[i].at <= m) idx = i;
    else break;
  }

  const cur = pts[idx];
  const next = pts[(idx + 1) % n];
  const curAt = cur.at;
  const nextAt = idx + 1 === n ? next.at + 1440 : next.at;
  const into = m >= curAt ? m - curAt : m + 1440 - curAt;
  const span = Math.max(0.001, nextAt - curAt);
  const tr = Math.min(transition, span);

  if (tr <= 0 || into < span - tr) return cur.kelvin;
  const t = clamp((into - (span - tr)) / tr, 0, 1);
  return cur.kelvin + (next.kelvin - cur.kelvin) * t;
}

const kx = (at) => (at / 1440) * CW;
const ky = (kelvin) => CTOP + (1 - (kelvin - KMIN) / (KMAX - KMIN)) * (CBOT - CTOP);

/** 当前在编辑的那条曲线（还没拖动时） */
function basePoints() {
  const s = S.schedule;
  if (s.mode === "sun") return s.effectivePoints || s.points || [];
  if (editingDay === "weekend") {
    return s.weekendPoints && s.weekendPoints.length ? s.weekendPoints : s.points || [];
  }
  return s.points || [];
}

function editPoints() {
  return draftPoints || basePoints();
}

function commitPoints(points) {
  if (!S || S.schedule.mode === "sun") return;
  const sorted = [...points]
    .map((p) => ({
      at: clamp(Math.round(p.at), 0, 1439),
      kelvin: clamp(Math.round(p.kelvin), KMIN, KMAX),
    }))
    .sort((a, b) => a.at - b.at);

  if (editingDay === "weekend") {
    send("eyecare/config", { schedule: { weekendPoints: sorted } });
  } else {
    send("eyecare/config", { schedule: { points: sorted } });
  }
}

/* ------------------------------------------------------------------ 渲染 */

function presetName(id) {
  const p = (S.presets || []).find((x) => x.id === id);
  return p ? p.name : "自定义";
}

// 开关是普通按钮，不靠 checkbox 的 change 事件——沙箱里那套隐式关联不一定可靠
function setSwitch(id, on) {
  const node = el(id);
  node.classList.toggle("on", on);
  node.setAttribute("aria-checked", String(on));
}

/** 统一出口：把失败暴露到界面上，否则出问题只会表现为「点了没反应」 */
async function send(method, params) {
  try {
    S = await api.invoke(method, params);
    render();
    return true;
  } catch (err) {
    el("sub").textContent = `调用失败：${err?.message || err}`;
    return false;
  }
}

function render() {
  if (!S) return;

  const mode = el("mode");
  if (!S.enabled) {
    mode.textContent = "已暂停";
    mode.className = "pill off";
  } else if (S.schedule.enabled) {
    mode.textContent = S.schedule.mode === "sun" ? "跟随日光" : "自动";
    mode.className = "pill";
  } else {
    mode.textContent = "手动";
    mode.className = "pill off";
  }

  const parts = [presetName(S.preset)];
  if (!S.enabled) {
    parts.push("已暂停");
  } else if (S.effective?.kind === "kelvin") {
    parts.push(`${S.effective.kelvin}K`);
  } else if (S.effective?.kind === "rgb") {
    parts.push("滤色");
  }
  parts.push(`${S.displays?.length || 0} 块屏`);
  el("sub").textContent = parts.join(" · ");

  el("enabled").classList.toggle("on", !!S.enabled);
  el("enabled").setAttribute("aria-checked", String(!!S.enabled));
  el("kelvin").value = S.kelvin;
  renderHero(S.effective);
  el("brightness").value = Math.round(S.brightness * 100);
  el("brightnessVal").textContent = `${Math.round(S.brightness * 100)}%`;
  if (S.color) el("color").value = rgbToHex(S.color);

  setSwitch("auto", !!S.schedule.enabled);
  el("transition").value = S.schedule.transition;

  setSwitch("breakOn", !!S.break.enabled);
  setSwitch("dimScreen", !!S.break.dimScreen);
  el("workMinutes").value = S.break.workMinutes;

  renderSupportNote();
  renderTarget();
  renderPresets();
  renderHdrNote();
  renderModeSeg();
  renderCurve();
  syncAnchorInputs();
  renderDayNote();
  renderStats();
}

function renderModeSeg() {
  const mode = S.schedule.mode || "clock";
  for (const btn of el("modeSeg").querySelectorAll("button")) {
    btn.classList.toggle("active", btn.dataset.mode === mode);
  }
  const isSun = mode === "sun";
  el("daySeg").classList.toggle("disabled", isSun);
  el("anchorTime").disabled = isSun;
  el("anchorK").disabled = isSun;
  if (isSun) {
    el("anchorHint").textContent = "日出日落模式下，时刻由太阳决定";
  } else {
    el("anchorHint").textContent = "点曲线上的圆点选中，拖动可改时间和色温";
  }
}

/// 平台本身改不了屏幕时，先把原因说清楚，再让用户去拖滑杆。
/// 否则「拖了没反应」会被当成 bug，其实可能是 Wayland 或者没有图形会话。
function renderSupportNote() {
  const note = el("supportNote");
  const support = S.support;

  if (!support || support.available) {
    note.classList.add("hidden");
    return;
  }

  note.textContent = support.reason || "当前平台无法调整屏幕色温。";
  note.classList.remove("hidden");
}

/// HDR 开着时 Windows 会绕过 Gamma 表，调整写了也是白写。
/// 这个必须提前说清楚，否则用户会以为是插件坏了。
function renderHdrNote() {
  const note = el("hdrNote");
  const affected = (S.displays || []).filter(
    (d) => (S.target === "all" || S.target === d.id) && d.hdr === true
  );

  if (affected.length === 0 || !S.enabled) {
    note.classList.add("hidden");
    return;
  }

  const names = affected.map((d) => d.name).join("、");
  note.textContent = `${names} 开启了 HDR，Windows 会绕过 Gamma 表，这些屏幕上的色温调整不会生效。`;
  note.classList.remove("hidden");
}

// 显示器列表不常变，只有变了才重建 option
function renderTarget() {
  const sel = el("target");
  const list = S.displays || [];
  const want = ["all", ...list.map((d) => d.id)].join("|");
  const have = [...sel.options].map((o) => o.value).join("|");

  if (want !== have) {
    sel.innerHTML = "";
    const all = document.createElement("option");
    all.value = "all";
    all.textContent = "全部屏幕";
    sel.appendChild(all);
    for (const d of list) {
      const option = document.createElement("option");
      option.value = d.id;
      option.textContent = `${d.name} ${d.width}×${d.height}${d.hdr ? " · HDR" : ""}`;
      sel.appendChild(option);
    }
  }

  sel.value = S.target || "all";
}

/// 左上角那个大数字。色温模式下是「4500 K」，滤色模式下换成色块加「滤色」。
/// 不跟着 effective 变的话，用户在滤色预设下看到数字没动，会以为没切过去。
function renderHero(eff) {
  const hero = el("heroValue");
  const slider = el("kelvin");

  if (eff && eff.kind === "rgb") {
    hero.classList.add("filtered");
    hero.textContent = "";

    const dot = document.createElement("span");
    dot.className = "hero-dot";
    dot.style.background = rgbToHex(eff.rgb);

    const label = document.createElement("i");
    label.textContent = "滤色";

    hero.append(dot, label);
    slider.classList.add("dimmed");
    slider.title = "当前是滤色模式，拖动会切回色温";
    return;
  }

  hero.classList.remove("filtered");
  slider.classList.remove("dimmed");
  slider.title = "";

  // 结构只在需要时重建，平时只改文字，免得拖动滑杆时反复造节点
  if (!hero.querySelector("#kelvinNum")) {
    hero.textContent = "";
    const num = document.createElement("span");
    num.id = "kelvinNum";
    const unit = document.createElement("i");
    unit.textContent = "K";
    hero.append(num, unit);
  }

  const shown = eff && eff.kind === "kelvin" ? eff.kelvin : S.kelvin;
  hero.querySelector("#kelvinNum").textContent = shown;
}

function renderPresets() {
  const wrap = el("presets");
  wrap.innerHTML = "";
  for (const p of S.presets || []) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "preset" + (S.preset === p.id ? " active" : "") + (p.custom ? " custom" : "");
    btn.title = p.hint || p.name;

    const sw = document.createElement("span");
    sw.className = "sw";
    sw.style.background = `linear-gradient(150deg, ${shade(p.swatch, 1.16)}, ${p.swatch} 46%, ${shade(p.swatch, 0.82)})`;

    const nm = document.createElement("span");
    nm.className = "nm";
    nm.textContent = p.name;

    btn.append(sw, nm);
    btn.addEventListener("click", () => send("eyecare/preset", { id: p.id }));

    if (p.custom) {
      const del = document.createElement("button");
      del.type = "button";
      del.className = "preset-del";
      del.title = "删除这个预设";
      del.textContent = "×";
      del.addEventListener("click", (e) => {
        e.stopPropagation();
        send("eyecare/preset.delete", { id: p.id });
      });
      btn.appendChild(del);
    }

    wrap.appendChild(btn);
  }
}

/* ---------------------------------------------------------------- 曲线绘制 */

function renderCurve() {
  if (!S) return;

  const points = editPoints();
  const transition = S.schedule.transition || 0;
  const isSun = S.schedule.mode === "sun";

  renderGrid();

  // 曲线：固定每 5 分钟取一个点，和拖动预览用的是同一套算法
  let d = "";
  for (let m = 0; m <= 1440; m += 5) {
    const k = sampleKelvin(points, m, transition);
    d += `${m === 0 ? "M" : "L"}${kx(m).toFixed(2)} ${ky(k).toFixed(2)} `;
  }
  el("curveLine").setAttribute("d", d);
  el("curveArea").setAttribute("d", `${d}L${CW} ${CBOT} L0 ${CBOT} Z`);

  // 锚点
  const dots = el("curveDots");
  dots.innerHTML = "";
  points.forEach((p, i) => {
    const c = document.createElementNS(SVG_NS, "circle");
    c.setAttribute("cx", kx(p.at));
    c.setAttribute("cy", ky(p.kelvin));
    c.setAttribute("r", i === selectedAnchor ? 6 : 4.5);
    c.setAttribute("class", "anchor-dot" + (i === selectedAnchor ? " active" : "") + (isSun ? " locked" : ""));
    c.dataset.index = String(i);
    if (!isSun) c.addEventListener("pointerdown", onDotDown);
    dots.appendChild(c);
  });

  // 当前时刻的竖线
  const now = minutesNow();
  el("curveNow").setAttribute("x1", kx(now).toFixed(2));
  el("curveNow").setAttribute("x2", kx(now).toFixed(2));
  el("curveNow").setAttribute("class", "curve-now" + (S.schedule.enabled ? " live" : ""));
}

function renderGrid() {
  const grid = el("curveGrid");
  grid.innerHTML = "";

  const add = (tag, attrs) => {
    const node = document.createElementNS(SVG_NS, tag);
    for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
    grid.appendChild(node);
    return node;
  };

  // 横向参考线：只看大概区间，不标数值
  for (const k of [2000, 3500, 5000]) {
    add("line", {
      x1: 0,
      y1: ky(k).toFixed(1),
      x2: CW,
      y2: ky(k).toFixed(1),
      class: "grid-h",
    });
  }

  // 竖向时间线
  for (let h = 0; h <= 24; h += 6) {
    const x = kx(h * 60);
    add("line", { x1: x, y1: CTOP, x2: x, y2: CBOT, class: "grid-v" });
    if (h > 0 && h < 24) {
      const label = add("text", { x, y: 9, class: "grid-label", "text-anchor": "middle" });
      label.textContent = `${h}:00`;
    }
  }

  // 日出日落标记，只在日出模式里有意义
  const times = S.schedule.sunTimes;
  if (S.schedule.mode === "sun" && times) {
    for (const [key, caption] of [["sunrise", "日出"], ["sunset", "日落"]]) {
      const x = kx(times[key]);
      add("line", { x1: x, y1: CTOP, x2: x, y2: CBOT, class: "grid-sun" });
      const label = add("text", { x: x + 3, y: CH - 3, class: "grid-sun-label" });
      label.textContent = `${caption} ${fmtTime(times[key])}`;
    }
  }
}

function syncAnchorInputs() {
  const points = editPoints();
  const p = points[selectedAnchor];
  if (!p) return;
  el("anchorTime").value = fmtTime(p.at);
  el("anchorK").value = p.kelvin;
}

function renderDayNote() {
  const note = el("dayNote");
  const isSun = S.schedule.mode === "sun";
  const seg = el("daySeg");

  for (const btn of seg.querySelectorAll("button")) {
    btn.classList.toggle("active", btn.dataset.day === editingDay);
  }

  note.classList.remove("clickable");
  if (isSun) {
    note.textContent = "日光模式不分工作日和周末";
    return;
  }

  if (editingDay !== "weekend") {
    note.textContent = "";
    return;
  }

  const own = (S.schedule.weekendPoints || []).length > 0;
  if (own) {
    note.textContent = "恢复跟随工作日";
    note.classList.add("clickable");
  } else {
    note.textContent = "跟随工作日，拖动任意圆点即可单独设置";
  }
}

/* ---------------------------------------------------------------- 锚点拖动 */

function pointerToCurve(event) {
  const rect = el("curve").getBoundingClientRect();
  const x = ((event.clientX - rect.left) / rect.width) * CW;
  const y = ((event.clientY - rect.top) / rect.height) * CH;
  return { x, y };
}

function onDotDown(event) {
  if (S.schedule.mode === "sun") return;
  event.preventDefault();

  selectedAnchor = Number(event.currentTarget.dataset.index);
  dragging = { index: selectedAnchor };

  window.addEventListener("pointermove", onDragMove);
  window.addEventListener("pointerup", onDragUp, { once: true });

  renderCurve();
  syncAnchorInputs();
}

function onDragMove(event) {
  if (!dragging) return;

  const { x, y } = pointerToCurve(event);
  const points = editPoints().map((p) => ({ ...p }));
  const index = Math.min(dragging.index, points.length - 1);
  if (!points[index]) return;

  points[index] = {
    at: clamp(Math.round((x / CW) * 1440 / 5) * 5, 0, 1435),
    kelvin: clamp(Math.round(((1 - (y - CTOP) / (CBOT - CTOP)) * (KMAX - KMIN) + KMIN) / 50) * 50, KMIN, KMAX),
  };

  draftPoints = points;
  renderCurve();
  syncAnchorInputs();
}

function onDragUp() {
  window.removeEventListener("pointermove", onDragMove);
  if (!dragging) return;

  const points = draftPoints;
  const index = dragging.index;
  dragging = null;
  draftPoints = null;

  if (!points) return;

  // 排序后选中项跟着走，否则松手会跳到别的点上
  const target = points[index];
  const sorted = [...points].sort((a, b) => a.at - b.at);
  const nextIndex = sorted.findIndex((p) => p.at === target.at && p.kelvin === target.kelvin);
  if (nextIndex >= 0) selectedAnchor = nextIndex;

  commitPoints(points);
}

/* ------------------------------------------------------------------ 统计 */

function renderStats() {
  const stats = S.stats;
  if (!stats) return;

  const today = stats.today || { seconds: 0, breaks: 0, longestStreakSeconds: 0 };
  const total = today.seconds || 0;

  el("todayHours").textContent = Math.floor(total / 3600);
  el("todayMins").textContent = Math.floor((total % 3600) / 60);
  el("todayMeta").textContent =
    total < 60
      ? "今天还没有记录"
      : `休息 ${today.breaks} 次 · 最长连续 ${fmtDuration(today.longestStreakSeconds || 0)}`;

  const streak = stats.streakSeconds || 0;
  const pill = el("streakPill");
  if (!S.enabled) {
    pill.textContent = "已暂停";
    pill.className = "pill off";
  } else if (streak < 60) {
    pill.textContent = "刚休息过";
    pill.className = "pill off";
  } else {
    pill.textContent = `${fmtDuration(streak)} 连续`;
    // 超过一个工作周期还没休息，用高亮提醒一下
    pill.className = streak >= S.break.workMinutes * 60 ? "pill" : "pill off";
  }

  const week = stats.week || [];
  const max = Math.max(1, ...week.map((d) => d.seconds || 0));
  const wrap = el("weekBars");
  wrap.innerHTML = "";

  week.forEach((day, i) => {
    const col = document.createElement("div");
    col.className = "week-col" + (i === week.length - 1 ? " today" : "");
    col.title = `${day.date} · ${fmtDuration(day.seconds || 0)}`;

    const track = document.createElement("div");
    track.className = "week-track";

    const fill = document.createElement("div");
    fill.className = "week-fill";
    fill.style.height = `${day.seconds ? Math.max(6, ((day.seconds || 0) / max) * 100) : 0}%`;
    track.appendChild(fill);

    const cap = document.createElement("span");
    cap.className = "week-day";
    cap.textContent = "日一二三四五六"[new Date(day.date + "T00:00:00").getDay()];

    col.append(track, cap);
    wrap.appendChild(col);
  });
}

/* ---------------------------------------------------------------- 倒计时 */

function updateClock() {
  if (!S) return;

  const plan = S.break;
  if (!plan.enabled || plan.remainingSeconds == null) {
    el("breakTime").textContent = "--:--";
    el("breakBar").style.width = "0%";
    el("breakCap").textContent = "提醒已关闭";
    return;
  }

  careRemain = Math.max(0, careRemain - 1);
  el("breakTime").textContent = fmtClock(careRemain);
  el("breakCap").textContent = S.break.resting ? "正在休息" : "后提醒休息";

  const total = Math.max(1, plan.workMinutes * 60);
  el("breakBar").style.width = `${Math.min(100, (careRemain / total) * 100)}%`;

  if (careRemain === 0) {
    showBanner(plan.restSeconds || 20);
    careRemain = plan.workMinutes * 60;
  }
}

function showBanner(seconds) {
  bannerLeft = seconds;
  el("banner").classList.remove("hidden");
  el("bannerCount").textContent = bannerLeft;
  el("breakCap").textContent = "正在休息";
}

/* -------------------------------------------------------------- 交互绑定 */

function debounce(fn) {
  clearTimeout(pushTimer);
  pushTimer = setTimeout(fn, 120);
}

function bind() {
  el("target").addEventListener("change", (e) =>
    send("eyecare/config", { target: e.target.value })
  );

  el("color").addEventListener("input", (e) => {
    const v = e.target.value;
    const rgb = [1, 3, 5].map((i) => parseInt(v.slice(i, i + 2), 16));
    send("eyecare/tint", { color: rgb });
  });

  el("enabled").addEventListener("click", async () => {
    const next = !S.enabled;
    // 先动界面，反馈立刻可见；失败会在 send 里把错误写到副标题
    setSwitch("enabled", next);
    await send("eyecare/config", { enabled: next });
  });

  el("kelvin").addEventListener("input", (e) => {
    const v = Number(e.target.value);
    // 滤色模式下大数字是色块，一拖滑杆就切回色温显示和色温取值
    renderHero({ kind: "kelvin", kelvin: v });
    debounce(() => send("eyecare/tint", { kelvin: v }));
  });

  el("brightness").addEventListener("input", (e) => {
    const v = Number(e.target.value);
    el("brightnessVal").textContent = `${v}%`;
    debounce(() => send("eyecare/tint", { brightness: v / 100 }));
  });

  el("auto").addEventListener("click", async () => {
    const next = !S.schedule.enabled;
    setSwitch("auto", next);
    await send("eyecare/config", { schedule: { enabled: next } });
  });

  el("modeSeg").addEventListener("click", (e) => {
    const btn = e.target.closest("button[data-mode]");
    if (!btn || btn.dataset.mode === S.schedule.mode) return;
    send("eyecare/config", { schedule: { mode: btn.dataset.mode } });
  });

  el("daySeg").addEventListener("click", (e) => {
    const btn = e.target.closest("button[data-day]");
    if (!btn || S.schedule.mode === "sun") return;
    editingDay = btn.dataset.day;
    selectedAnchor = 0;
    draftPoints = null;
    renderCurve();
    syncAnchorInputs();
    renderDayNote();
  });

  // 「恢复跟随工作日」：清掉周末曲线，回到和工作日一样
  el("dayNote").addEventListener("click", () => {
    if (!el("dayNote").classList.contains("clickable")) return;
    send("eyecare/config", { schedule: { weekendPoints: [] } });
  });

  el("transition").addEventListener("change", (e) =>
    send("eyecare/config", { schedule: { transition: Number(e.target.value) } })
  );

  el("anchorTime").addEventListener("change", (e) => {
    const points = editPoints().map((p) => ({ ...p }));
    if (!points[selectedAnchor]) return;
    points[selectedAnchor] = { ...points[selectedAnchor], at: toMinutes(e.target.value) };
    commitPoints(points);
  });

  el("anchorK").addEventListener("change", (e) => {
    const points = editPoints().map((p) => ({ ...p }));
    if (!points[selectedAnchor]) return;
    points[selectedAnchor] = {
      ...points[selectedAnchor],
      kelvin: clamp(Number(e.target.value) || KMIN, KMIN, KMAX),
    };
    commitPoints(points);
  });

  el("savePreset").addEventListener("click", () => {
    const form = el("presetForm");
    form.classList.toggle("hidden");
    if (!form.classList.contains("hidden")) {
      el("presetName").value = "";
      el("presetName").focus();
    }
  });

  const doSave = async () => {
    const name = el("presetName").value.trim();
    el("presetForm").classList.add("hidden");
    await send("eyecare/preset.save", name ? { name } : {});
  };

  el("presetConfirm").addEventListener("click", doSave);
  el("presetName").addEventListener("keydown", (e) => {
    if (e.key === "Enter") doSave();
    if (e.key === "Escape") el("presetForm").classList.add("hidden");
  });

  el("breakOn").addEventListener("click", async () => {
    const next = !S.break.enabled;
    setSwitch("breakOn", next);
    await send("eyecare/break", { enabled: next });
    careRemain = S.break.remainingSeconds ?? 0;
  });

  el("dimScreen").addEventListener("click", async () => {
    const next = !S.break.dimScreen;
    setSwitch("dimScreen", next);
    await send("eyecare/break", { dimScreen: next });
  });

  el("workMinutes").addEventListener("change", async (e) => {
    await send("eyecare/break", { workMinutes: Number(e.target.value) });
    careRemain = S.break.remainingSeconds ?? 0;
  });

  el("skip").addEventListener("click", async () => {
    await send("eyecare/break", { action: "skip" });
    careRemain = S.break.remainingSeconds ?? 0;
    el("banner").classList.add("hidden");
    el("breakCap").textContent = "后提醒休息";
  });

  el("bannerClose").addEventListener("click", () => {
    el("banner").classList.add("hidden");
    el("breakCap").textContent = "后提醒休息";
  });
}

/* ---------------------------------------------------------------- 启动 */

const APPEARANCES = new Set(["light", "dark"]);

// DBX 传下来的 theme 是 { appearance, tokens }，不是字符串；老宿主才可能直接给字符串
function readAppearance(theme) {
  if (typeof theme === "function") theme = theme();
  if (typeof theme === "string") return APPEARANCES.has(theme) ? theme : null;
  const appearance = theme?.appearance;
  return APPEARANCES.has(appearance) ? appearance : null;
}

// 宿主最近一次推下来的外观。有了它就不要再回读 plugin.theme——
// 那个字段只在初始化时给一次，切主题走的是事件，回读会拿到旧值。
let knownAppearance = null;

// 以宿主为准，宿主没给才退回系统设置——之前的 bug 就是这里看反了，
// 明明 DBX 是浅色，却因为 Windows 是深色把面板染成了深色
function currentAppearance() {
  return (
    knownAppearance ||
    readAppearance(window.dbxPlugin?.theme) ||
    readAppearance(document.documentElement.dataset.dbxTheme) ||
    (window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light")
  );
}

function applyTheme(appearance) {
  const next = appearance || currentAppearance();
  knownAppearance = next;
  // color-scheme 交给 CSS 的 :root / [data-theme="light"] 管，这里只切属性
  document.documentElement.dataset.theme = next;
}

function watchTheme() {
  // 宿主切主题时派发 dbx-plugin-env：正式宿主派在 document 上，开发宿主派在 window 上
  const onEnv = (event) => {
    const detail = event?.detail || {};
    const next = readAppearance(detail.theme) || readAppearance(detail.appearance);
    if (next) applyTheme(next);
  };
  document.addEventListener("dbx-plugin-env", onEnv);
  window.addEventListener("dbx-plugin-env", onEnv);

  // 没有宿主时（浏览器直接打开）跟着系统走
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (readAppearance(window.dbxPlugin?.theme)) return;
    knownAppearance = null;
    applyTheme(null);
  });
}

async function boot() {
  api = await connectHost();
  // 等握手完再读一次：theme 是在 init 消息里带过来的，早读会读到空值
  applyTheme();
  watchTheme();

  const tag = el("hostMode");
  tag.textContent = api.mode === "real" ? "DBX" : "MOCK";
  tag.className = `tag ${api.mode}`;

  S = await api.invoke("eyecare/state");
  careRemain = S.break.remainingSeconds ?? 0;
  bind();
  render();

  api.onEvent?.((event) => {
    const method = event?.method || event?.event;
    const params = event?.params || event?.data || {};
    if (method === "evt/break.due") {
      showBanner(params.restSeconds || S.break.restSeconds || 20);
      careRemain = S.break.workMinutes * 60;
    }
  });

  // 每秒跑一次倒计时，每 15 秒跟后端对一次表（后台可能已经重新计时）
  setInterval(updateClock, 1000);
  setInterval(async () => {
    try {
      S = await api.invoke("eyecare/state");
      render();
    } catch {
      // 宿主正在重启侧车时会短暂失败，下一轮自然恢复
    }
  }, 15000);

  // 横幅自己的秒数
  setInterval(() => {
    if (el("banner").classList.contains("hidden")) return;
    bannerLeft -= 1;
    if (bannerLeft <= 0) {
      el("banner").classList.add("hidden");
      el("breakCap").textContent = "后提醒休息";
    } else {
      el("bannerCount").textContent = bannerLeft;
    }
  }, 1000);

  // 曲线上的「现在」要跟着走，但不必每秒重绘整个 SVG
  setInterval(() => {
    if (!S || !S.schedule.enabled) return;
    const now = minutesNow();
    el("curveNow").setAttribute("x1", kx(now).toFixed(2));
    el("curveNow").setAttribute("x2", kx(now).toFixed(2));
  }, 30000);
}

boot().catch((err) => {
  el("sub").textContent = `加载失败：${err.message}`;
});

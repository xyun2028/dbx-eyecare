// Eye眼 · DBX 插件前端
//
// 宿主把界面跑在沙箱 iframe 里：没有网络、没有磁盘、不能起进程，
// 唯一通道是 window.dbxPlugin.invoke / onEvent。所以这里只做两件事：
// 显示状态、把用户操作转成 RPC。
//
// 没有宿主时（浏览器直接打开）会启用一个内存里的演示后端，
// 交互都能跑，页脚会标 MOCK。

const el = (id) => document.getElementById(id);

const MOCK_PRESETS = [
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
      dayKelvin: 5500,
      nightKelvin: 3000,
      dayStart: 420,
      nightStart: 1140,
      transition: 60,
    },
    breakPlan: { enabled: true, workMinutes: 20, restSeconds: 20, dueAt: Date.now() + 20 * 60000 },
  };

  const displays = [{ id: "mock-1", name: "演示屏幕", width: 1920, height: 1080, primary: true, hdr: false }];
  const listeners = new Set();

  const effective = () => {
    if (!cfg.enabled) return null;
    if (cfg.schedule.enabled) return { kind: "kelvin", kelvin: cfg.schedule.dayKelvin };
    if (cfg.preset === "custom" && cfg.color) return { kind: "rgb", rgb: cfg.color };
    const p = MOCK_PRESETS.find((x) => x.id === cfg.preset);
    if (p?.color) return { kind: "rgb", rgb: p.color };
    return { kind: "kelvin", kelvin: p?.kelvin ?? cfg.kelvin };
  };

  const payload = () => ({
    enabled: cfg.enabled,
    preset: cfg.preset,
    kelvin: cfg.kelvin,
    brightness: cfg.brightness,
    color: cfg.color,
    target: cfg.target,
    presets: MOCK_PRESETS,
    displays,
    effective: effective(),
    schedule: cfg.schedule,
    break: {
      enabled: cfg.breakPlan.enabled,
      workMinutes: cfg.breakPlan.workMinutes,
      restSeconds: cfg.breakPlan.restSeconds,
      remainingSeconds: cfg.breakPlan.enabled
        ? Math.max(0, Math.round((cfg.breakPlan.dueAt - Date.now()) / 1000))
        : null,
    },
    mock: true,
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
        if (params.schedule) Object.assign(cfg.schedule, params.schedule);
        return payload();
      }

      if (method === "eyecare/preset") {
        const p = MOCK_PRESETS.find((x) => x.id === params.id);
        if (p) {
          cfg.preset = p.id;
          cfg.enabled = true;
          if (p.kelvin) cfg.kelvin = p.kelvin;
          cfg.color = p.color || null;
        }
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
          cfg.breakPlan.dueAt = params.enabled
            ? Date.now() + cfg.breakPlan.workMinutes * 60000
            : 0;
        }
        if (typeof params.workMinutes === "number") {
          cfg.breakPlan.workMinutes = params.workMinutes;
          cfg.breakPlan.dueAt = Date.now() + params.workMinutes * 60000;
        }
        if (params.action === "skip") {
          cfg.breakPlan.dueAt = Date.now() + cfg.breakPlan.workMinutes * 60000;
        }
        return payload();
      }

      throw new Error(`mock 没有实现 ${method}`);
    },
  };
}

/* ------------------------------------------------------------------ 渲染 */

function presetName(id) {
  const p = (S.presets || []).find((x) => x.id === id);
  return p ? p.name : "自定义";
}

function fmtClock(seconds) {
  const s = Math.max(0, seconds | 0);
  const m = Math.floor(s / 60);
  return `${String(m).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

function rgbToHex(rgb) {
  return "#" + rgb.map((n) => Math.round(n).toString(16).padStart(2, "0")).join("");
}

function fmtTime(minutes) {
  const h = Math.floor(minutes / 60) % 24;
  return `${String(h).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;
}

function toMinutes(text) {
  const [h, m] = text.split(":").map(Number);
  return (h || 0) * 60 + (m || 0);
}

// 开关是普通按钮，不靠 checkbox 的 change 事件——沙箱里那套隐式关联不一定可靠
function setSwitch(id, on) {
  const node = el(id);
  node.classList.toggle("on", on);
  node.setAttribute("aria-checked", String(on));
}

/// 统一出口：把失败暴露到界面上，否则出问题只会表现为「点了没反应」
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
    mode.textContent = "自动";
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
  el("kelvinNum").textContent = S.kelvin;
  el("brightness").value = Math.round(S.brightness * 100);
  el("brightnessVal").textContent = `${Math.round(S.brightness * 100)}%`;
  if (S.color) el("color").value = rgbToHex(S.color);

  setSwitch("auto", !!S.schedule.enabled);
  el("dayStart").value = fmtTime(S.schedule.dayStart);
  el("nightStart").value = fmtTime(S.schedule.nightStart);
  el("dayK").value = S.schedule.dayKelvin;
  el("nightK").value = S.schedule.nightKelvin;
  el("transition").value = S.schedule.transition;

  setSwitch("breakOn", !!S.break.enabled);
  el("workMinutes").value = S.break.workMinutes;

  renderTarget();
  renderPresets();
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

function renderPresets() {
  const wrap = el("presets");
  wrap.innerHTML = "";
  for (const p of S.presets || []) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "preset" + (S.preset === p.id ? " active" : "");

    const sw = document.createElement("span");
    sw.className = "sw";
    sw.style.background = `linear-gradient(150deg, ${shade(p.swatch, 1.16)}, ${p.swatch} 46%, ${shade(p.swatch, 0.82)})`;

    const nm = document.createElement("span");
    nm.className = "nm";
    nm.textContent = p.name;

    btn.append(sw, nm);
    btn.addEventListener("click", () => send("eyecare/preset", { id: p.id }));
    wrap.appendChild(btn);
  }
}

function shade(hex, factor) {
  const v = hex.replace("#", "");
  const rgb = [0, 2, 4].map((i) => parseInt(v.slice(i, i + 2), 16));
  const c = rgb.map((n) => Math.round(Math.max(0, Math.min(255, n * factor))));
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}

function updateClock() {
  if (!S) return;

  const plan = S.break;
  if (!plan.enabled || plan.remainingSeconds == null) {
    el("breakTime").textContent = "--:--";
    el("breakBar").style.width = "0%";
    return;
  }

  careRemain = Math.max(0, careRemain - 1);
  el("breakTime").textContent = fmtClock(careRemain);

  const total = Math.max(1, plan.workMinutes * 60);
  el("breakBar").style.width = `${Math.min(100, (careRemain / total) * 100)}%`;

  if (careRemain === 0) {
    showBanner(plan.restSeconds || 20);
    careRemain = plan.workMinutes * 60;
  }
}

let careRemain = 0;
let bannerLeft = 0;

function showBanner(seconds) {
  bannerLeft = seconds;
  el("banner").classList.remove("hidden");
  el("bannerCount").textContent = bannerLeft;
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
    el("kelvinNum").textContent = v;
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

  for (const [id, key] of [
    ["dayK", "dayKelvin"],
    ["nightK", "nightKelvin"],
    ["transition", "transition"],
  ]) {
    el(id).addEventListener("change", (e) =>
      send("eyecare/config", { schedule: { [key]: Number(e.target.value) } })
    );
  }

  el("dayStart").addEventListener("change", (e) =>
    send("eyecare/config", { schedule: { dayStart: toMinutes(e.target.value) } })
  );

  el("nightStart").addEventListener("change", (e) =>
    send("eyecare/config", { schedule: { nightStart: toMinutes(e.target.value) } })
  );

  el("breakOn").addEventListener("click", async () => {
    const next = !S.break.enabled;
    setSwitch("breakOn", next);
    await send("eyecare/break", { enabled: next });
    careRemain = S.break.remainingSeconds ?? 0;
  });

  el("workMinutes").addEventListener("change", async (e) => {
    await send("eyecare/break", { workMinutes: Number(e.target.value) });
    careRemain = S.break.remainingSeconds ?? 0;
  });

  el("skip").addEventListener("click", async () => {
    await send("eyecare/break", { action: "skip" });
    careRemain = S.break.remainingSeconds ?? 0;
    el("banner").classList.add("hidden");
  });

  el("bannerClose").addEventListener("click", () => el("banner").classList.add("hidden"));
}

/* ---------------------------------------------------------------- 启动 */

function applyTheme() {
  let theme = window.dbxPlugin?.theme;
  if (typeof theme === "function") theme = theme();
  const dark =
    theme === "dark" ||
    (theme !== "light" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

async function boot() {
  applyTheme();
  api = await connectHost();

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
    S = await api.invoke("eyecare/state");
    render();
  }, 15000);

  // 横幅自己的秒数
  setInterval(() => {
    if (el("banner").classList.contains("hidden")) return;
    bannerLeft -= 1;
    if (bannerLeft <= 0) {
      el("banner").classList.add("hidden");
    } else {
      el("bannerCount").textContent = bannerLeft;
    }
  }, 1000);
}

boot().catch((err) => {
  el("sub").textContent = `加载失败：${err.message}`;
});

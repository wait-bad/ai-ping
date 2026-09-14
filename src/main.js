import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ---------- 状态 ----------
let endpoints = [];          // [{id,name,baseUrl,apiKey,models[]}]
let currentId = null;        // 当前选中的接口 id
let results = {};            // { [model]: PingResult }
let running = false;         // 是否有测速进行中
let pending = new Set();     // 进行中的模型
let startTimes = {};         // { [model]: number } 记录前端发起测速的精确时刻
let uiElapsed = {};          // { [model]: number } 记录前端从点击到完成感知的完整时间
let liveTimer = null;        // 实时秒表刷新定时器
let appSettings = { timeoutSecs: 30, retryCount: 1 }; // 全局设置
const selByEp = {};          // { [epId]: Set } 勾选记忆

const $ = (s) => document.querySelector(s);
const el = {
  epList: $("#ep-list"), empty: $("#empty-state"), work: $("#work"),
  title: $("#work-title"), url: $("#work-url"), key: $("#work-key"),
  models: $("#models"), stats: $("#stats"), selCount: $("#sel-count"),
  chkAll: $("#chk-all"), pingAll: $("#btn-ping-all"),
  btnEdit: $("#btn-edit"), btnCopyEp: $("#btn-copy-ep"),
  mask: $("#modal-mask"), modalTitle: $("#modal-title"),
  fName: $("#f-name"), fUrl: $("#f-url"), fKey: $("#f-key"), fModels: $("#f-models"),
  btnDelete: $("#btn-delete"),
  // 设置弹层元素
  settingsMask: $("#settings-mask"),
  btnOpenSettings: $("#btn-open-settings"),
  fTimeout: $("#f-timeout"),
  fRetry: $("#f-retry"),
  btnSettingsCancel: $("#btn-settings-cancel"),
  btnSettingsSave: $("#btn-settings-save"),
};

const cur = () => endpoints.find((e) => e.id === currentId) || null;

// ---------- IPC ----------
const api = {
  list: () => invoke("list_endpoints"),
  save: (list) => invoke("save_endpoints_cmd", { list }),
  getSettings: () => invoke("get_settings"),
  saveSettings: (settings) => invoke("save_settings_cmd", { settings }),
  ping: (endpoint, models) => invoke("ping_models", { endpoint, models }),
};

// 监听后端推送的单个模型测速完成事件，立刻更新单个卡片
listen("ping://result", (ev) => {
  const r = ev.payload;
  if (!r || !r.model) return;
  const finishTime = performance.now();
  if (startTimes[r.model]) {
    uiElapsed[r.model] = Math.round(finishTime - startTimes[r.model]);
  }
  pending.delete(r.model);
  results[r.model] = r;
  const card = el.models.querySelector(`[data-model="${CSS.escape(r.model)}"]`);
  if (card) {
    renderCard(card, r.model, true);
  }
  renderStats();
  if (pending.size === 0) setRunning(false);
});

// ---------- 工具 ----------
const maskKey = (k) =>
  !k ? "未配置 Key" : k.length <= 10 ? k.slice(0, 2) + "···" : k.slice(0, 6) + "···" + k.slice(-4);

function grade(ms) {
  if (ms == null) return "";
  if (ms < 800) return "极速 ⚡";
  if (ms < 1800) return "优秀 ✨";
  if (ms < 3500) return "良好 👍";
  if (ms < 7000) return "偏慢 ⏳";
  return "极慢 🐢";
}

function fmt(ms) {
  if (ms == null) return "—";
  return ms >= 10000 ? (ms / 1000).toFixed(1) + "s" : Math.round(ms) + "";
}

// ---------- 渲染：侧栏 ----------
function renderSidebar() {
  el.epList.innerHTML = "";
  for (const ep of endpoints) {
    const li = document.createElement("li");
    li.className = "ep-item" + (ep.id === currentId ? " active" : "");
    li.innerHTML = `<span class="ep-dot"></span><span>
        <div class="ep-name"></div>
        <div class="ep-sub">${ep.models.length} 个模型</div>
      </span>`;
    li.querySelector(".ep-name").textContent = ep.name || "(未命名)";
    li.onclick = () => selectEp(ep.id);
    el.epList.appendChild(li);
  }
}

function selectEp(id) {
  if (running) return;
  currentId = id;
  selByEp[id] = selByEp[id] || new Set(cur()?.models || []);
  results = {};
  startTimes = {};
  uiElapsed = {};
  renderSidebar();
  renderWork();
}

// ---------- 渲染：工作区 ----------
function renderWork() {
  const ep = cur();
  el.empty.classList.toggle("hidden", !!ep);
  el.work.classList.toggle("hidden", !ep);
  if (!ep) return;
  el.title.textContent = ep.name || "(未命名)";
  el.url.textContent = ep.baseUrl;
  el.key.textContent = maskKey(ep.apiKey);
  el.models.innerHTML = "";
  for (const m of ep.models) {
    const card = document.createElement("div");
    card.className = "card s-idle";
    card.dataset.model = m;
    el.models.appendChild(card);
    renderCard(card, m, false);
  }
  renderStats();
}

function renderCard(card, model, justDone) {
  const ep = cur();
  if (!ep) return;
  const r = results[model];
  const isPending = pending.has(model);
  const state = isPending ? "run" : r ? (r.ok ? "ok" : "bad") : "idle";
  card.className = `card s-${state}` + (justDone ? " just" : "");
  const checked = selByEp[ep.id]?.has(model) ?? true;

  let body = "";
  if (state === "run") {
    const now = performance.now();
    const elapsed = startTimes[model] ? Math.max(0, Math.round(now - startTimes[model])) : 0;
    body = `<div class="ms-big"><span class="num live-num">${elapsed}</span><span class="unit">ms · 实时计时中</span></div>
            <div class="ms-sub">请求已发出，等待首字或首个 Token…</div>`;
  } else if (state === "idle") {
    body = `<div class="ms-big"><span class="num">—</span><span class="unit">ms · 首字</span></div>
            <div class="ms-sub">点「测这个」或右上角「全部测速」</div>`;
  } else if (r.ok) {
    const uiTotal = uiElapsed[model] || r.overallMs || r.firstCharMs;
    const diff = uiTotal > (r.firstCharMs + 400);

    const retryInfo = r.attempts > 1
      ? `<span class="retry-tag">重试第 ${r.attempts} 次成功 (总耗时 ${fmt(uiTotal)} ms)</span>`
      : diff
      ? `<span class="queue-tag" title="界面全流程耗时比单次首字长，通常因连接池排队或并发调度">界面总计 ${fmt(uiTotal)} ms</span>`
      : "";

    body = `<div class="ms-big"><span class="num">${fmt(r.firstCharMs)}</span><span class="unit">ms · ${grade(r.firstCharMs)}</span></div>
      <div class="ms-sub">
        <span>响应头 <b>${fmt(r.headerMs)} ms</b></span>
        <span>首字时延 <b>${fmt(r.firstCharMs)} ms</b></span>
        ${retryInfo}
      </div>
      ${r.sample ? `<div class="sample">首字片段 <code></code></div>` : ""}`;
  } else {
    body = `<div class="ms-big"><span class="x">✕ 失败</span></div>
      <div class="err"></div>`;
  }

  card.innerHTML = `<div class="top">
      <input type="checkbox" class="pick" ${checked ? "checked" : ""} title="选中后参与批量测速" />
      <span class="mname"></span>
      <span class="badge">${state === "run" ? "测试中" : state === "ok" ? "OK" : state === "bad" ? "FAIL" : "待测"}</span>
    </div>${body}
    <div class="act"><button class="link-btn">▶ 测这个</button></div>`;

  card.querySelector(".mname").textContent = model;
  if (r?.ok && r.sample) card.querySelector(".sample code").textContent = JSON.stringify(r.sample);
  if (r && !r.ok && r.error) card.querySelector(".err").textContent = r.error;

  card.querySelector(".pick").onchange = (e) => {
    const s = selByEp[ep.id];
    e.target.checked ? s.add(model) : s.delete(model);
    renderStats();
  };
  card.querySelector(".link-btn").onclick = () => startPing([model]);
}

function renderStats() {
  const ep = cur();
  if (!ep) return;
  const sel = selByEp[ep.id] || new Set();
  el.selCount.textContent = `已选 ${sel.size} / ${ep.models.length}`;
  el.chkAll.checked = sel.size === ep.models.length && ep.models.length > 0;

  const done = ep.models.map((m) => results[m]).filter(Boolean);
  const ok = done.filter((r) => r.ok && r.firstCharMs != null);
  const fail = done.filter((r) => !r.ok);
  const avg = ok.length ? ok.reduce((a, r) => a + r.firstCharMs, 0) / ok.length : null;
  const best = ok.length ? Math.min(...ok.map((r) => r.firstCharMs)) : null;
  el.stats.innerHTML = ok.length || fail.length
    ? `<span class="good">成功 <b>${ok.length}</b></span>
       ${fail.length ? `<span class="badn">失败 <b>${fail.length}</b></span>` : ""}
       <span>平均 <b>${fmt(avg)} ms</b></span>
       <span>最快 <b>${fmt(best)} ms</b></span>`
    : `<span class="dim">尚未测速</span>`;
}

// ---------- 实时跳秒秒表更新 ----------
function updateLiveStopwatch() {
  if (pending.size === 0) {
    if (liveTimer) {
      cancelAnimationFrame(liveTimer);
      liveTimer = null;
    }
    return;
  }
  const now = performance.now();
  pending.forEach((m) => {
    const card = el.models.querySelector(`[data-model="${CSS.escape(m)}"]`);
    if (card) {
      const numEl = card.querySelector(".live-num");
      if (numEl && startTimes[m]) {
        const ms = Math.round(now - startTimes[m]);
        numEl.textContent = ms + "";
      }
    }
  });
  liveTimer = requestAnimationFrame(updateLiveStopwatch);
}

// ---------- 测速 ----------
async function startPing(models) {
  const ep = cur();
  if (!ep || running || !models.length) return;
  const now = performance.now();
  for (const m of models) {
    delete results[m];
    delete uiElapsed[m];
    startTimes[m] = now;
  }
  pending = new Set(models);
  models.forEach((m) => {
    const card = el.models.querySelector(`[data-model="${CSS.escape(m)}"]`);
    if (card) renderCard(card, m, false);
  });
  setRunning(true);
  renderStats();

  if (!liveTimer) {
    liveTimer = requestAnimationFrame(updateLiveStopwatch);
  }

  try {
    const resList = await api.ping(ep, models);
    const finishTime = performance.now();
    // 兜底同步所有结果（以防偶发事件丢失）
    if (Array.isArray(resList)) {
      for (const r of resList) {
        if (r && r.model) {
          if (startTimes[r.model] && !uiElapsed[r.model]) {
            uiElapsed[r.model] = Math.round(finishTime - startTimes[r.model]);
          }
          results[r.model] = r;
          pending.delete(r.model);
          const card = el.models.querySelector(`[data-model="${CSS.escape(r.model)}"]`);
          if (card) renderCard(card, r.model, false);
        }
      }
    }
  } catch (err) {
    console.error("ping error:", err);
  } finally {
    setRunning(false);
    renderStats();
  }
}

function setRunning(v) {
  running = v;
  el.pingAll.disabled = v;
  el.pingAll.textContent = v ? "… 测速中" : "▶ 全部测速";
  if (!v && liveTimer) {
    cancelAnimationFrame(liveTimer);
    liveTimer = null;
  }
}

// ---------- 弹层（编辑接口） ----------
let editingId = null;

function openModal(ep, isDuplicate = false) {
  if (isDuplicate && ep) {
    editingId = null;
    el.modalTitle.textContent = "复制新建接口";
    el.fName.value = (ep.name || "") + "_新建";
    el.fUrl.value = ep.baseUrl || "";
    el.fKey.value = "";
    el.fModels.value = "";
    el.btnDelete.classList.add("hidden");
  } else {
    editingId = ep?.id ?? null;
    el.modalTitle.textContent = ep ? "编辑接口" : "新增接口";
    el.fName.value = ep?.name || "";
    el.fUrl.value = ep?.baseUrl || "";
    el.fKey.value = ep?.apiKey || "";
    el.fModels.value = (ep?.models || []).join("\n");
    el.btnDelete.classList.toggle("hidden", !ep);
  }
  el.mask.classList.remove("hidden");
  el.fName.focus();
}
const closeModal = () => el.mask.classList.add("hidden");

async function saveModal() {
  const name = el.fName.value.trim();
  const baseUrl = el.fUrl.value.trim();
  if (!name || !baseUrl) { alert("名称和 URL 不能为空哦"); return; }
  const models = [...new Set(
    el.fModels.value.split(/[\n,，]/).map((s) => s.trim()).filter(Boolean)
  )];
  if (!models.length) { alert("至少填写一个模型名"); return; }

  let list;
  if (editingId) {
    list = endpoints.map((e) =>
      e.id === editingId ? { ...e, name, baseUrl, apiKey: el.fKey.value.trim(), models } : e
    );
  } else {
    list = [...endpoints, {
      id: "ep" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6),
      name, baseUrl, apiKey: el.fKey.value.trim(), models,
    }];
  }
  await api.save(list);
  endpoints = list;
  currentId = editingId || list[list.length - 1].id;
  selByEp[currentId] = selByEp[currentId] || new Set(cur().models);
  results = {};
  startTimes = {};
  uiElapsed = {};
  closeModal();
  renderSidebar();
  renderWork();
}

async function deleteEp() {
  if (!editingId) return;
  if (!confirm("确定删除这个接口吗？")) return;
  const list = endpoints.filter((e) => e.id !== editingId);
  await api.save(list);
  endpoints = list;
  if (currentId === editingId) { currentId = list[0]?.id ?? null; results = {}; }
  closeModal();
  renderSidebar();
  renderWork();
}

// ---------- 弹层（全局设置） ----------
function openSettings() {
  el.fTimeout.value = appSettings.timeoutSecs ?? 30;
  el.fRetry.value = appSettings.retryCount ?? 1;
  el.settingsMask.classList.remove("hidden");
  el.fTimeout.focus();
}

const closeSettings = () => el.settingsMask.classList.add("hidden");

async function saveSettings() {
  let t = parseInt(el.fTimeout.value, 10);
  if (isNaN(t) || t < 1) {
    t = 30;
  }
  if (t > 300) {
    alert("超时时间最大支持 300 秒");
    return;
  }
  let r = parseInt(el.fRetry.value, 10);
  if (isNaN(r) || r < 0) {
    r = 0;
  }
  if (r > 10) {
    alert("重试次数最大支持 10 次");
    return;
  }

  appSettings = { timeoutSecs: t, retryCount: r };
  await api.saveSettings(appSettings);
  closeSettings();
}

// ---------- 事件绑定 ----------
$("#btn-add-ep").onclick = () => openModal(null);
el.btnEdit.onclick = () => openModal(cur());
el.btnCopyEp.onclick = () => {
  const currentEp = cur();
  if (currentEp) {
    openModal(currentEp, true);
  }
};
$("#btn-cancel").onclick = closeModal;
$("#btn-save").onclick = saveModal;
el.btnDelete.onclick = deleteEp;
el.mask.onclick = (e) => e.target === el.mask && closeModal();

// 设置面板事件
el.btnOpenSettings.onclick = openSettings;
el.btnSettingsCancel.onclick = closeSettings;
el.btnSettingsSave.onclick = saveSettings;
el.settingsMask.onclick = (e) => e.target === el.settingsMask && closeSettings();

el.chkAll.onchange = () => {
  const ep = cur();
  if (!ep) return;
  selByEp[ep.id] = el.chkAll.checked ? new Set(ep.models) : new Set();
  ep.models.forEach((m) => {
    const card = el.models.querySelector(`[data-model="${CSS.escape(m)}"]`);
    if (card) card.querySelector(".pick").checked = el.chkAll.checked;
  });
  renderStats();
};

el.pingAll.onclick = () => {
  const ep = cur();
  if (!ep) return;
  const sel = [...(selByEp[ep.id] || [])];
  startPing(sel.length ? sel : ep.models);
};

// ---------- 启动 ----------
(async () => {
  [endpoints, appSettings] = await Promise.all([
    api.list().catch(() => []),
    api.getSettings().catch(() => ({ timeoutSecs: 30, retryCount: 1 })),
  ]);
  if (endpoints.length) {
    currentId = endpoints[0].id;
    selByEp[currentId] = new Set(endpoints[0].models);
  }
  renderSidebar();
  renderWork();
})();

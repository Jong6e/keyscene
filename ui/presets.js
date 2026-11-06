// 단축키 list (the main view): one card per preset, its step list, the "동작 추가" dialog and per-step forms.
// On screen a preset is called "단축키"; in code it stays `preset`.
// Uses components.js and app.js (loaded first). Step JSON follows src/preset.rs.

let presets = [];
let openId = null;       // the one expanded card (accordion)
const nameDrafts = {};   // preset id -> name typed but not saved yet
const chipErrors = {};   // preset id -> why a shortcut was refused
const cardErrors = {};   // preset id -> last run/save error
const busy = new Set();  // preset ids with a run/restore in flight
const confirming = new Set(); // preset ids showing the delete confirmation
let drag = null;
let pendingData = null;  // data that arrived during a drag

// ---------- step catalog ----------

const CATALOG = [
  { cat: "앱 · 파일 · 웹", items: [
    { key: "open_app", label: "앱 열기…", form: "app", make: () => ({ kind: "open_app", app: null, if_running: "front", monitor: { kind: "app_default" }, window: "app_default", on_restore: "keep" }) },
    { key: "open_path", label: "파일·폴더 열기…", form: "path", make: () => ({ kind: "open_path", path: "" }) },
    { key: "open_url", label: "웹사이트 열기…", form: "url", make: () => ({ kind: "open_url", url: "", browser: { browser: "default", profile_dir: "", profile_name: "" }, new_window: false, private: false }) },
    { key: "app_windows", label: "앱 최소화·닫기…", form: "app_windows", make: () => ({ kind: "app_windows", app: null, op: "minimize" }) },
  ] },
  { cat: "창", items: [
    { key: "show_desktop", label: "바탕 화면 보기", make: () => ({ kind: "show_desktop" }) },
    { key: "active_window:minimize", label: "현재 창 최소화", make: () => ({ kind: "active_window", op: "minimize" }) },
    { key: "active_window:maximize", label: "현재 창 최대화", make: () => ({ kind: "active_window", op: "maximize" }) },
    { key: "active_window:topmost", label: "현재 창 맨 위 고정", make: () => ({ kind: "active_window", op: "topmost" }) },
  ] },
  { cat: "소리 · 미디어", items: [
    { key: "speaker_mute", label: "스피커 음소거…", desc: "음소거 · 음소거 해제 · 전환", form: "bool", make: () => ({ kind: "speaker_mute", mute: true, toggle: false }) },
    { key: "speaker_volume", label: "음량 맞추기…", form: "volume", make: () => ({ kind: "speaker_volume", percent: 30 }) },
    { key: "mic_mute", label: "마이크 음소거…", desc: "음소거 · 음소거 해제 · 전환", form: "bool", make: () => ({ kind: "mic_mute", mute: true, toggle: false }) },
    { key: "media:play_pause", label: "재생/일시 중지", make: () => ({ kind: "media", key: "play_pause" }) },
    { key: "media:next", label: "다음 곡", make: () => ({ kind: "media", key: "next" }) },
    { key: "media:previous", label: "이전 곡", make: () => ({ kind: "media", key: "previous" }) },
  ] },
  { cat: "화면 · 탐색기", items: [
    { key: "desktop_icons", label: "바탕 화면 아이콘 숨기기·표시…", desc: "숨기기 · 표시 · 전환", form: "bool", make: () => ({ kind: "desktop_icons", hide: true, toggle: false }) },
    { key: "hidden_items", label: "숨긴 항목 표시·숨기기…", desc: "표시 · 숨기기 · 전환", form: "bool", make: () => ({ kind: "hidden_items", show: true, toggle: false }) },
    { key: "selection_hidden", label: "선택한 항목 숨기기/숨김 해제", desc: "탐색기·바탕 화면에서 선택한 항목",
      make: () => ({ kind: "selection_hidden" }) },
    { key: "folder_tree", label: "하위 항목까지 숨기기…", desc: "폴더 안 전체를 한 번에", form: "folder_tree",
      make: () => ({ kind: "folder_tree", path: "", hide: true, include_self: true, selected: true }) },
    { key: "theme", label: "어둡게·밝게 모드…", desc: "어둡게 · 밝게 · 전환", form: "bool", make: () => ({ kind: "theme", dark: true, toggle: false }) },
    { key: "monitor_off", label: "모니터 끄기", make: () => ({ kind: "monitor_off" }) },
  ] },
  { cat: "시스템", items: [
    { key: "lock", label: "PC 잠금", make: () => ({ kind: "lock" }) },
    { key: "sleep", label: "절전", make: () => ({ kind: "sleep" }) },
    { key: "clear_clipboard", label: "클립보드 비우기", make: () => ({ kind: "clear_clipboard" }) },
    { key: "screenshot", label: "화면 캡처", make: () => ({ kind: "screenshot" }) },
  ] },
  { cat: "입력 · 흐름", items: [
    { key: "send_keys", label: "단축키 보내기…", form: "keys", make: () => ({ kind: "send_keys", spec: "" }) },
    { key: "type_text", label: "글자 입력…", form: "text", make: () => ({ kind: "type_text", text: "" }) },
    { key: "wait", label: "기다리기…", form: "wait", make: () => ({ kind: "wait", ms: 1000 }) },
  ] },
];
const ALL_ITEMS = CATALOG.flatMap((c) => c.items);

function itemFor(step) {
  const sub = step.op ?? (step.kind === "media" ? step.key : null);
  return ALL_ITEMS.find((i) => i.key === `${step.kind}:${sub}`) ?? ALL_ITEMS.find((i) => i.key === step.kind);
}

// Wording for the ↩ mark: pressing the shortcut again puts this back ("원래대로", one term everywhere).
const RESTORE_TIP = "다시 누르면 원래대로";
const RESTORE_LABEL = "다시 누르면 원래대로";
const NEW_NAME = "새 단축키"; // name of a card that has none

/// Same rule as Step::restorable in src/preset.rs. The "전환" (toggle: true) forms of the five
/// on/off steps are restorable too; folder_tree and selection_hidden are not.
function isRestorable(step) {
  if (step.kind === "app_windows") return step.op === "minimize";
  if (step.kind === "open_app") return (step.on_restore ?? "keep") !== "keep";
  return ["show_desktop", "active_window", "speaker_mute", "speaker_volume", "mic_mute",
    "desktop_icons", "hidden_items", "theme"].includes(step.kind);
}

// Fixed texts for the option values (also used by the forms).
const IF_RUNNING = [{ value: "front", label: "맨 앞으로 가져오기" }, { value: "new", label: "하나 더 열기" }];
const WINDOW_STATES = [
  { value: "app_default", label: "앱 기본" }, { value: "maximized", label: "최대화" }, { value: "minimized", label: "최소화" },
  { value: "left_half", label: "왼쪽 절반" }, { value: "right_half", label: "오른쪽 절반" },
];
const ON_RESTORE = [{ value: "keep", label: "그대로 둠" }, { value: "minimize", label: "최소화" }, { value: "close", label: "닫기" }];
const CLOSE_NOTE = "저장하지 않은 내용이 사라질 수 있습니다.";
const label = (list, v) => list.find((o) => o.value === v)?.label ?? v;

function monitorText(m) {
  if (!m || m.kind === "app_default") return null;
  if (m.kind === "primary") return "주 모니터";
  if (m.kind === "cursor") return "마우스가 있는 모니터";
  return m.name || "지정한 모니터";
}

function browserText(b) {
  if (!b || b.browser === "default") return "기본 브라우저";
  const name = b.browser === "edge" ? "Edge" : "Chrome";
  return b.profile_name ? `${name} · ${b.profile_name}` : name;
}

function urlHost(u) {
  if (!u) return "주소 없음";
  try { return new URL(u).host || u; } catch { return u; }
}

function baseName(p) {
  return (p || "").split(/[\\/]/).filter(Boolean).pop() || p || "";
}

/// Title line for a step in the list.
function stepTitle(s) {
  switch (s.kind) {
    case "open_app": return `앱 열기 · ${s.app?.name ?? "앱을 고르세요"}`;
    case "open_path": return `열기 · ${baseName(s.path) || "경로 없음"}`;
    case "open_url": return `웹사이트 열기 · ${urlHost(s.url)}`;
    case "app_windows": return `${s.app?.name ?? "앱"} ${s.op === "close" ? "닫기" : "최소화"}`;
    case "show_desktop": return "바탕 화면 보기";
    case "active_window": return { minimize: "현재 창 최소화", maximize: "현재 창 최대화", topmost: "현재 창 맨 위 고정" }[s.op];
    case "speaker_mute": return s.toggle ? "스피커 음소거 전환" : s.mute ? "스피커 음소거" : "스피커 음소거 해제";
    case "speaker_volume": return `음량 ${s.percent}%로 맞추기`;
    case "mic_mute": return s.toggle ? "마이크 음소거 전환" : s.mute ? "마이크 음소거" : "마이크 음소거 해제";
    case "media": return { play_pause: "재생/일시 중지", next: "다음 곡", previous: "이전 곡" }[s.key];
    case "desktop_icons": return s.toggle ? "바탕 화면 아이콘 전환" : s.hide ? "바탕 화면 아이콘 숨기기" : "바탕 화면 아이콘 표시";
    case "hidden_items": return s.toggle ? "숨긴 항목 전환" : s.show ? "숨긴 항목 표시" : "숨긴 항목 숨기기";
    case "selection_hidden": return "선택한 항목 숨기기/숨김 해제";
    case "folder_tree": return s.hide ? "하위 항목까지 숨기기" : "하위 항목까지 숨김 해제";
    case "theme": return s.toggle ? "어둡게·밝게 모드 전환" : s.dark ? "어둡게 모드로 전환" : "밝게 모드로 전환";
    case "monitor_off": return "모니터 끄기";
    case "lock": return "PC 잠금";
    case "sleep": return "절전";
    case "clear_clipboard": return "클립보드 비우기";
    case "screenshot": return "화면 캡처";
    case "send_keys": return `단축키 보내기 · ${s.spec ? specLabel(s.spec) : "키 없음"}`;
    case "type_text": { const t = s.text ?? ""; return `글자 입력 · “${t.length > 18 ? t.slice(0, 18) + "…" : t}”`; }
    case "wait": return `${((s.ms ?? 0) / 1000).toLocaleString("ko-KR", { maximumFractionDigits: 1 })}초 기다리기`;
    default: return "알 수 없는 동작";
  }
}

/// Second line under the title (or null).
function stepSub(s) {
  switch (s.kind) {
    case "open_app": {
      const parts = [monitorText(s.monitor), s.window !== "app_default" ? label(WINDOW_STATES, s.window) : null,
        s.if_running === "new" ? "하나 더 열기" : null].filter(Boolean);
      return parts.length ? parts.join(" · ") : null;
    }
    case "open_path": return s.path || null;
    case "folder_tree": return s.selected ? "탐색기에서 선택한 폴더" : s.path || null;
    case "open_url": return [(s.browser?.browser ?? "default") === "default" ? null : browserText(s.browser), s.new_window ? "새 창" : null,
      s.private ? (s.browser?.browser === "edge" ? "InPrivate" : "시크릿") : null].filter(Boolean).join(" · ");
    case "show_desktop": return "모든 창 내리기";
    case "app_windows": return s.op === "close" ? CLOSE_NOTE : null;
    default: return null;
  }
}

/// Short form for the collapsed card summary.
function stepShort(s) {
  return stepTitle(s).replace(/^앱 열기 · /, "").replace(/로 맞추기$/, "").replace(/로 전환$/, "");
}

// ---------- data ----------

function applyData(data) {
  if (drag) { pendingData = data; return; }
  presets = data.presets.map((p) => (nameDrafts[p.id] != null ? { ...p, name: nameDrafts[p.id] } : p));
  renderList();
}

async function loadPresets() {
  try {
    applyData(await invoke("get_presets"));
    showListError(null);
  } catch (err) {
    showListError(`단축키를 불러오지 못했습니다. ${err}`);
  }
}

function showListError(text) {
  $("presets-error").hidden = !text;
  $("presets-error").textContent = text ?? "";
}

// Saves run one after another per preset, so an older answer never overwrites a newer edit.
const saveChains = {};
function savePreset(p) {
  const body = { id: p.id, name: (nameDrafts[p.id] ?? p.name).trim() || NEW_NAME, steps: p.steps,
    revert_after_min: p.revert_after_min ?? null };
  const run = async () => {
    try {
      const data = await invoke("save_preset", { preset: body });
      delete cardErrors[p.id];
      applyData(data);
    } catch (err) {
      cardErrors[p.id] = String(err);
      renderList();
    }
  };
  saveChains[p.id] = (saveChains[p.id] ?? Promise.resolve()).then(run);
  return saveChains[p.id];
}

/// Edits a preset locally, redraws, and saves.
function update(id, change) {
  const p = presets.find((x) => x.id === id);
  if (!p) return;
  const next = { ...p, steps: p.steps.map((s) => structuredClone(s)) }; // change() may also set revert_after_min
  change(next);
  presets = presets.map((x) => (x.id === id ? next : x));
  renderList();
  savePreset(next);
}

const nameTimers = {};
const nameQueued = {}; // preset id -> last typed name handed to the save queue
function onNameInput(id, value) {
  nameDrafts[id] = value;
  const p = presets.find((x) => x.id === id);
  if (p) p.name = value;
  const title = document.querySelector(`.preset[data-id="${CSS.escape(id)}"] .preset-name`);
  if (title) title.textContent = value.trim() || NEW_NAME;
  clearTimeout(nameTimers[id]);
  nameTimers[id] = setTimeout(() => saveName(id), 400);
}

/// Saves the typed name now (debounce, Enter, leaving the field, the window losing focus).
/// Goes through savePreset's queue, so saves stay in order; the same value is not saved twice.
function saveName(id) {
  clearTimeout(nameTimers[id]);
  delete nameTimers[id];
  const value = nameDrafts[id];
  const current = presets.find((x) => x.id === id);
  if (value == null || !current || nameQueued[id] === value) return;
  nameQueued[id] = value;
  savePreset(current).then(() => {
    if (cardErrors[id]) delete nameQueued[id]; // failed: let the next attempt try again
    if (nameDrafts[id] === value) delete nameDrafts[id];
  });
}

/// Saves every name whose save is still waiting on its timer.
function flushNames() {
  for (const id of Object.keys(nameTimers)) saveName(id);
}

/// Enter saves the name now; Esc puts back the last saved name.
function onNameKey(e, id) {
  if (e.key === "Enter") {
    e.preventDefault();
    saveName(id);
  } else if (e.key === "Escape" && nameDrafts[id] != null) {
    e.preventDefault();
    clearTimeout(nameTimers[id]);
    delete nameTimers[id];
    delete nameDrafts[id];
    delete nameQueued[id];
    loadPresets();
  }
}

async function createPreset() {
  const before = new Set(presets.map((p) => p.id));
  $("preset-new").disabled = true;
  try {
    const data = await invoke("save_preset", { preset: { id: "", name: NEW_NAME, steps: [], revert_after_min: null } });
    const added = data.presets.find((p) => !before.has(p.id));
    applyData(data);
    showListError(null);
    if (added) {
      setOpen(added.id);
      const input = document.querySelector(`[data-key="name:${CSS.escape(added.id)}"]`);
      input?.focus();
      input?.select();
    }
  } catch (err) {
    showListError(`단축키를 만들지 못했습니다. ${err}`);
  } finally {
    $("preset-new").disabled = false;
  }
}

async function runPreset(id) {
  if (busy.has(id) || presets.find((x) => x.id === id)?.busy) return;
  busy.add(id);
  delete cardErrors[id];
  renderList();
  try {
    applyData(await invoke("run_preset", { id }));
  } catch (err) {
    cardErrors[id] = String(err);
  } finally {
    busy.delete(id);
    renderList();
  }
}

const deleting = new Set();
async function deletePreset(id) {
  if (deleting.has(id)) return; // a second click while the first is on its way
  deleting.add(id);
  try {
    confirming.delete(id);
    if (openId === id) openId = null;
    applyData(await invoke("delete_preset", { id }));
    $("preset-new").focus();
  } catch (err) {
    cardErrors[id] = String(err);
    renderList();
  } finally {
    deleting.delete(id);
  }
}

// ---------- transient UI state ----------

/// Drops half-finished UI (delete confirmations, open lists, recording chips) and saves typed names now.
function resetTransient() {
  Combo.current?.close(false);
  confirming.clear();
  clearWarning(false);
  if (recordingAction) stopRecording();
  flushNames();
}

const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/// Opens one card (closing the others) or closes it; scrolls the opened card into view.
function setOpen(id) {
  resetTransient();
  openId = id;
  renderList();
  if (!id) return;
  const card = document.querySelector(`.preset[data-id="${CSS.escape(id)}"]`);
  if (!card) return;
  const r = card.getBoundingClientRect();
  const top = document.querySelector(".topbar").getBoundingClientRect().bottom;
  if (r.top < top || r.bottom > window.innerHeight) {
    card.scrollIntoView({ block: r.height > window.innerHeight - top ? "start" : "nearest", behavior: reducedMotion() ? "auto" : "smooth" });
  }
}

// ---------- rendering ----------

function renderList() {
  // Keep focus and caret across redraws (e.g. while typing a name).
  const active = document.activeElement;
  const key = active?.dataset?.key;
  const sel = key && "selectionStart" in active ? [active.selectionStart, active.selectionEnd] : null;

  const list = $("preset-list");
  if (Combo.current && list.contains(Combo.current.root)) Combo.current.close(false);
  list.replaceChildren(...(presets.length
    ? presets.map(renderCard)
    : [h("p", { class: "empty-note" }, "단축키가 없습니다. +로 추가하세요.")]));

  placeWarning(); // before restoring focus: it may hold the focused button
  if (key) {
    const again = list.querySelector(`[data-key="${CSS.escape(key)}"]`);
    if (again) {
      again.focus({ preventScroll: true });
      if (sel && again.setSelectionRange) try { again.setSelectionRange(...sel); } catch {}
    }
  }
}

function renderCard(p) {
  const open = openId === p.id;
  const name = p.name || NEW_NAME;
  const action = `preset:${p.id}`;
  const bodyId = `preset-body-${p.id}`;
  const chip = h("button", { type: "button", class: "chip", "data-action": action, "data-key": `chip:${p.id}` });
  const recording = recordingAction === action;
  if (recording) {
    chip.classList.add("recording");
    chip.textContent = "키를 누르세요…";
  } else paintChip(chip, p.hotkey?.spec, p.hotkey?.label ?? "");
  const chipError = recording ? null : chipErrors[p.id] ?? p.hotkey?.error;
  const helpText = recording ? RECORD_HINT : chipError;
  // Not shown when it only repeats the name (a one-step card named after its step).
  const summary = p.steps.length ? p.steps.map(stepShort).join(" → ") : "동작 없음";

  const head = h("div", { class: "preset-head" },
    h("div", { class: "preset-title" },
      h("span", { class: "preset-name", title: name }, name),
      p.running ? h("span", { class: "preset-status" }, runningText(p)) : null,
      open || summary === name.trim() ? null : h("span", { class: "sub summary" }, summary)),
    chip,
    h("button", {
      type: "button", class: "icon-button", "aria-expanded": open ? "true" : "false", "aria-controls": open ? bodyId : null,
      "aria-label": `${name} ${open ? "접기" : "펼치기"}`, "data-key": `toggle:${p.id}`,
      onclick: () => setOpen(open ? null : p.id),
    }, icon(open ? "chevronUp" : "chevronDown")));

  const card = h("article", { class: `card preset${p.running ? " running" : ""}${open ? " open" : ""}`, "data-id": p.id, "aria-label": name },
    head,
    h("p", { class: `help${recording ? " hint" : ""}`, "data-help": action, hidden: !helpText }, helpText ?? ""));

  if (!open) return card;

  const nameId = `name-${p.id}`;
  const body = h("div", { class: "preset-body", id: bodyId },
    h("div", { class: "field" },
      h("label", { for: nameId }, "이름"),
      h("input", { id: nameId, type: "text", value: p.name ?? "", maxlength: "60", "data-key": `name:${p.id}`,
        placeholder: NEW_NAME, oninput: (e) => onNameInput(p.id, e.target.value), onkeydown: (e) => onNameKey(e, p.id),
        onchange: () => saveName(p.id) })),
    p.steps.length
      ? h("ol", { class: "steps", "aria-label": `${name}의 동작` }, p.steps.map((s, i) => renderStep(p, s, i)))
      : h("div", { class: "steps-empty" },
        h("span", { class: "step-title" }, "아직 동작이 없습니다")),
    h("button", { type: "button", class: "add-step", "data-key": `add:${p.id}`, onclick: () => openPicker(p.id) },
      icon("plus"), "동작 추가"),
    // Only worth offering when something would be undone.
    p.steps.some(isRestorable) ? revertRow(p) : null);

  const error = cardErrors[p.id] ?? p.last_error;
  // Busy here (a click) or in the backend (the shortcut): no second run, no delete until it ends.
  const isBusy = busy.has(p.id) || !!p.busy;
  const foot = confirming.has(p.id)
    ? h("div", { class: "preset-foot" },
      h("span", { class: "foot-note strong" }, "이 단축키를 삭제할까요?"),
      h("button", { type: "button", class: "button danger", "data-key": `del-yes:${p.id}`, disabled: isBusy, onclick: () => deletePreset(p.id) }, "삭제"),
      h("button", { type: "button", class: "button", "data-key": `del-no:${p.id}`, onclick: () => { confirming.delete(p.id); renderList(); } }, "취소"))
    : h("div", { class: "preset-foot" },
      h("span", { class: "spacer" }),
      h("button", { type: "button", class: "button subtle-danger", "data-key": `del:${p.id}`, disabled: isBusy,
        onclick: () => {
          // Nothing to lose in a card without steps: delete right away. Otherwise ask first.
          if (p.steps.length === 0) return deletePreset(p.id);
          confirming.add(p.id);
          renderList();
          document.querySelector(`[data-key="del-no:${CSS.escape(p.id)}"]`)?.focus();
        } }, "삭제"),
      h("button", {
        type: "button", class: "button primary", "data-key": `run:${p.id}`,
        disabled: isBusy || (!p.running && p.steps.length === 0),
        onclick: () => runPreset(p.id),
      }, isBusy ? (p.running ? "원래대로 돌리는 중…" : "실행 중…") : p.running ? "원래대로" : "실행"));

  // Element.append(null) would print the text "null", so only real nodes are appended.
  card.append(body);
  if (error) card.append(h("p", { class: "help standalone", role: "alert" }, error));
  card.append(foot);
  return card;
}

/// "60" -> "1시간", "8" -> "8분".
const minutesText = (n) => (n >= 60 && n % 60 === 0 ? `${n / 60}시간` : `${n}분`);

/// Status line of a card that ran and has something to undo.
function runningText(p) {
  const left = p.revert_left_min;
  if (left == null) return "실행 중";
  return `실행 중 · ${left <= 0 ? "곧 끝남" : `${minutesText(left)} 남음`}`;
}

const REVERT_OPTIONS = [
  { value: null, label: "계속" }, { value: 5, label: "5분" }, { value: 10, label: "10분" },
  { value: 15, label: "15분" }, { value: 30, label: "30분" }, { value: 60, label: "1시간" },
];

/// "유지 시간: 계속 / N분" under the steps: after that long the shortcut goes back by itself.
function revertRow(p) {
  const labelId = `revert-label-${p.id}`;
  const el = h("div");
  // A fresh copy: the combo adds a missing value (e.g. 20 written by hand in config.json) to its list.
  const combo = new Combo(el, REVERT_OPTIONS.map((o) => ({ ...o })),
    (v) => update(p.id, (x) => { x.revert_after_min = v; }),
    { labelledBy: labelId, formatMissing: (v) => minutesText(v) });
  combo.setValue(p.revert_after_min ?? null);
  combo.button.dataset.key = `revert:${p.id}`; // focus comes back here after the redraw
  return h("div", { class: "option-row revert-row" },
    h("span", { class: "label" }, h("span", { id: labelId }, "유지 시간")),
    el);
}

/// Picture for steps that do not point at an app or file (ICONS names in components.js).
const KIND_GLYPH = {
  open_url: "globe", show_desktop: "desktop", active_window: "window", speaker_mute: "speaker",
  speaker_volume: "speaker", mic_mute: "mic", media: "media", desktop_icons: "grid", hidden_items: "eye",
  theme: "theme", monitor_off: "monitorOff", lock: "lock", sleep: "moon", clear_clipboard: "clipboard",
  screenshot: "capture", send_keys: "keyboard", type_text: "text", wait: "clock", selection_hidden: "selection",
};

/// Every step row starts with a 24px picture so the text lines up: the app's or file's own icon
/// where the step points at one, otherwise a glyph for the kind of step.
function stepIcon(s) {
  switch (s.kind) {
    case "open_app": case "app_windows":
      if (!s.app) return glyph("window");
      return fileIcon(`${s.app.source === "file" ? "path" : "app"}:${s.app.id}`, 24, () => tile(s.app.name || "?"));
    case "open_path": {
      // Without an icon: a folder glyph for names without an extension, else a file glyph.
      const fallback = () => glyph(/\.[^.\\/]+$/.test(baseName(s.path)) ? "file" : "folder");
      return fileIcon(s.path ? `path:${s.path}` : null, 24, fallback);
    }
    case "folder_tree": return fileIcon(!s.selected && s.path ? `path:${s.path}` : null, 24, () => glyph("folder"));
    default: return glyph(KIND_GLYPH[s.kind] ?? "file");
  }
}

function renderStep(p, s, i) {
  const item = itemFor(s);
  const hasForm = !!item?.form;
  const title = stepTitle(s);
  const sub = stepSub(s);
  const text = [stepIcon(s), h("span", { class: "step-text" },
    h("span", { class: "step-title", title }, title), sub ? h("span", { class: "sub", title: sub }, sub) : null)];

  const li = h("li", { class: "step", "data-index": String(i),
    onkeydown: (e) => {
      if (!e.altKey || !["ArrowUp", "ArrowDown"].includes(e.key)) return;
      e.preventDefault();
      moveStep(p.id, i, i + (e.key === "ArrowUp" ? -1 : 1), e.target.dataset.key?.split(":")[0]);
    } },
    h("button", { type: "button", class: "handle", "data-key": `handle:${p.id}:${i}`,
      "aria-label": `${i + 1}번째 동작 ${title}. 끌거나 Alt+위·아래 화살표로 순서 바꾸기`,
      onpointerdown: (e) => startDrag(e, p.id, i) }, icon("grip")),
    h("span", { class: "num", "aria-hidden": "true" }, String(i + 1)),
    // Every step can be changed: with settings -> its form; without -> pick another action in place.
    h("button", { type: "button", class: "step-body", "data-key": `edit:${p.id}:${i}`,
      "aria-label": hasForm ? `${title} 편집` : `${title}, 다른 동작으로 바꾸기`,
      onclick: () => (hasForm ? openStepForm(p.id, i) : openPicker(p.id, i)) }, text),
    isRestorable(s) ? h("span", { class: "restore-mark", role: "img", "aria-label": RESTORE_LABEL, title: RESTORE_TIP, html: ICONS.restore }) : null,
    h("button", { type: "button", class: "icon-button small", "data-key": `remove:${p.id}:${i}`, "aria-label": `${title} 제거`, title: "제거",
      // Ignore the 2nd click of a double-click: the next row has moved under the pointer.
      onclick: (e) => { if (e.detail <= 1) removeStep(p.id, i); } }, icon("close")));

  if (s.kind === "open_app") {
    const labelId = `restore-label-${p.id}-${i}`;
    const { el, combo } = comboField(ON_RESTORE, s.on_restore ?? "keep",
      (v) => update(p.id, (x) => { x.steps[i].on_restore = v; }), labelId);
    combo.button.dataset.key = `restore:${p.id}:${i}`; // focus comes back here after the redraw
    li.append(h("div", { class: "step-option" },
      h("span", { id: labelId, class: "option-label" }, "원래대로 할 때 이 앱은"), el,
      s.on_restore === "close" ? h("span", { class: "note warn" }, CLOSE_NOTE) : null));
  }
  return li;
}

/// One click removes; the toast offers 실행 취소 (or Ctrl+Z) to put it back at the same place.
function removeStep(id, i) {
  const p = presets.find((x) => x.id === id);
  if (!p) return;
  const removed = structuredClone(p.steps[i]);
  update(id, (x) => x.steps.splice(i, 1));
  // Focus stays on the row that moved up; after the last row it goes to "동작 추가".
  const focusAdd = () => document.querySelector(`[data-key="add:${CSS.escape(id)}"]`);
  if (!document.activeElement || document.activeElement === document.body) focusAdd()?.focus();
  Toast.show(`‘${stepTitle(removed)}’ 동작을 제거했습니다.`, {
    focusAfter: focusAdd,
    undo: () => {
      if (!presets.some((x) => x.id === id)) return;
      if (openId !== id) setOpen(id);
      update(id, (x) => x.steps.splice(Math.min(i, x.steps.length), 0, removed));
      document.querySelector(`[data-key="edit:${CSS.escape(id)}:${Math.min(i, presets.find((x) => x.id === id).steps.length - 1)}"]`)?.focus();
    },
  });
}

function moveStep(id, from, to, focusKind) {
  const p = presets.find((x) => x.id === id);
  if (!p || to < 0 || to >= p.steps.length || from === to) return;
  update(id, (x) => { const [s] = x.steps.splice(from, 1); x.steps.splice(to, 0, s); });
  document.querySelector(`[data-key="${focusKind ?? "handle"}:${CSS.escape(id)}:${to}"]`)?.focus();
}

// ---------- drag to reorder ----------

function startDrag(e, id, index) {
  if (e.button !== 0) return;
  const li = e.currentTarget.closest(".step");
  const ol = li.parentElement;
  e.currentTarget.setPointerCapture(e.pointerId);
  drag = { id, index, target: index, li, ol, startY: e.clientY, handle: e.currentTarget, moved: false };
  li.classList.add("dragging");
  e.currentTarget.addEventListener("pointermove", onDragMove);
  e.currentTarget.addEventListener("pointerup", endDrag, { once: true });
  e.currentTarget.addEventListener("pointercancel", endDrag, { once: true });
}

function onDragMove(e) {
  if (!drag) return;
  const dy = e.clientY - drag.startY;
  if (Math.abs(dy) > 3) drag.moved = true;
  drag.li.style.transform = `translateY(${dy}px)`;
  const rows = [...drag.ol.children];
  // The dragged row lands after every other row whose middle is above the pointer.
  let target = 0;
  rows.forEach((row, n) => {
    if (n === drag.index) return;
    const r = row.getBoundingClientRect();
    if (e.clientY > r.top + r.height / 2) target++;
  });
  drag.target = target;
  rows.forEach((row, n) => {
    row.classList.toggle("drop-before", drag.moved && n === drag.target && drag.target < drag.index);
    row.classList.toggle("drop-after", drag.moved && n === drag.target && drag.target > drag.index);
  });
}

function endDrag() {
  if (!drag) return;
  const { id, index, target, handle, moved } = drag;
  handle.removeEventListener("pointermove", onDragMove);
  drag = null;
  if (pendingData) { const d = pendingData; pendingData = null; applyData(d); }
  if (moved && target !== index) moveStep(id, index, target);
  else renderList();
}

// ---------- 동작 추가 dialog ----------

/// replaceIndex = null: add a step at the end; a number: replace that step in place.
function openPicker(presetId, replaceIndex = null) {
  loadApps().catch(() => {}); // warm the app list so the 앱 열기 form opens filled
  const searchId = nextId("search");
  const search = h("input", { id: searchId, type: "search", placeholder: "동작 검색 (예: 음량, 잠금)", "aria-label": "동작 검색", autofocus: true,
    oninput: () => filter(search.value),
    // Enter picks the first match; ↓ moves into the list.
    onkeydown: (e) => {
      const first = [...document.querySelectorAll(".picker-item")].find((b) => !b.hidden && !b.closest("[hidden]"));
      if (e.key === "Enter" && first) { e.preventDefault(); first.click(); }
      else if (e.key === "ArrowDown" && first) { e.preventDefault(); first.focus(); }
    } });
  const p = presets.find((x) => x.id === presetId);
  const currentKey = replaceIndex != null && p?.steps[replaceIndex] ? itemFor(p.steps[replaceIndex])?.key : null;
  const groups = CATALOG.map((c) => {
    const buttons = c.items.map((item) => {
      const sample = item.make();
      const current = item.key === currentKey;
      return h("button", { type: "button", class: `picker-item${current ? " current" : ""}`, "data-search": `${c.cat} ${item.label} ${item.desc ?? ""}`,
        title: item.desc ?? null, "aria-description": item.desc ?? null, "aria-current": current ? "true" : null, onclick: () => choose(item) },
        h("span", { class: "picker-label" }, item.label),
        current ? h("span", { class: "current-tag" }, "현재") : null,
        isRestorable(sample) ? h("span", { class: "restore-mark", role: "img", "aria-label": RESTORE_LABEL, title: RESTORE_TIP, html: ICONS.restore }) : null);
    });
    const section = h("section", { class: "picker-group" }, h("h4", {}, c.cat), h("div", { class: "picker-grid" }, buttons));
    return { section, buttons };
  });
  const empty = h("p", { class: "empty-note", hidden: true }, "찾는 동작이 없습니다.");

  function filter(q) {
    const words = q.trim().toLowerCase().split(/\s+/).filter(Boolean);
    let any = false;
    for (const g of groups) {
      let shown = 0;
      for (const b of g.buttons) {
        const hit = words.every((w) => b.dataset.search.toLowerCase().includes(w));
        b.hidden = !hit;
        if (hit) shown++;
      }
      g.section.hidden = shown === 0;
      any ||= shown > 0;
    }
    empty.hidden = any;
  }

  function choose(item) {
    const step = item.make();
    if (!item.form) {
      update(presetId, (x) => { replaceIndex == null ? x.steps.push(step) : (x.steps[replaceIndex] = step); });
      Dialog.close();
      return;
    }
    showStepForm(presetId, replaceIndex, step, true);
  }

  Dialog.open({
    title: `${replaceIndex == null ? "동작 추가" : "동작 바꾸기"} · ${p?.name || NEW_NAME}`,
    wide: true,
    body: h("div", { class: "picker" },
      h("div", { class: "search" }, icon("search"), search),
      h("p", { class: "legend" }, h("span", { class: "restore-mark", html: ICONS.restore, "aria-hidden": "true" }), "다시 누르면 원래대로"),
      groups.map((g) => g.section),
      empty,
      h("p", { class: "note" }, "없는 동작은 ", h("b", {}, "단축키 보내기"), "로. 예: Win + Ctrl + → (다음 가상 데스크톱)")),
  });
}

function openStepForm(presetId, index) {
  const p = presets.find((x) => x.id === presetId);
  if (!p) return;
  showStepForm(presetId, index, structuredClone(p.steps[index]), false);
}

/// Shows the form for `step`. index null = new step. fromPicker = reached through the action list.
function showStepForm(presetId, index, step, fromPicker) {
  const item = itemFor(step);
  const form = FORMS[item.form](step);
  const error = h("p", { class: "form-error", role: "alert", hidden: true });
  const body = h("div", { class: "form" }, form.body, error);
  const ready = () => form.ready?.() ?? true;
  const commit = () => {
    if (!ready()) return;
    const r = form.read();
    if (r.error) {
      // Show the message right under the field it is about.
      const near = r.focus?.closest(".field, .option-row, .chooser-panel, .keys-block");
      if (near) near.after(error);
      else body.append(error);
      error.hidden = false;
      error.textContent = r.error;
      r.focus?.focus();
      return;
    }
    update(presetId, (x) => { index == null ? x.steps.push(r.step) : (x.steps[index] = r.step); });
    form.dispose?.();
    Dialog.close();
  };
  const primary = h("button", { type: "button", class: "button primary", onclick: commit }, index == null ? "추가" : "저장");
  // Windows dialogs: the main button comes first, then 취소.
  const footer = [
    fromPicker
      ? h("button", { type: "button", class: "button", onclick: () => { form.dispose?.(); openPicker(presetId, index); } }, icon("back"), "뒤로")
      : h("button", { type: "button", class: "button subtle", onclick: () => { form.dispose?.(); openPicker(presetId, index); } }, "다른 동작으로 바꾸기"),
    h("span", { class: "spacer" }),
    primary,
    fromPicker ? null : h("button", { type: "button", class: "button", onclick: () => { form.dispose?.(); Dialog.close(); } }, "취소"),
  ].filter(Boolean);
  const content = { title: item.label.replace(/…$/, ""), body, footer, guard: true };
  if (Dialog.el) Dialog.replace(content);
  else Dialog.open({ ...content, wide: true, onClose: () => form.dispose?.() });
  if (Dialog.el) Dialog.onClose = () => form.dispose?.();
  // The main button is only available when the form has what it needs; a message clears once edited.
  const refresh = () => { primary.disabled = !ready(); };
  const edited = () => { error.hidden = true; refresh(); };
  for (const type of ["input", "change", "click", "keyup"]) body.addEventListener(type, type === "click" || type === "keyup" ? refresh : edited);
  refresh();
  form.onChange = refresh;
  body.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target.tagName === "INPUT" && e.target.type !== "checkbox") { e.preventDefault(); commit(); }
  });
}

// ---------- step forms ----------

function field(labelText, control, hint) {
  const id = control.id || (control.id = nextId("f"));
  return h("div", { class: "field" }, h("label", { for: id }, labelText), control, hint ? h("span", { class: "sub" }, hint) : null);
}

/// Puts a path chosen in a Windows dialog into `input` like typing would (the form re-checks, a message clears).
function setPicked(input, value) {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function optionRow(labelText, control, extra) {
  const id = nextId("opt");
  control.setAttribute?.("aria-labelledby", id);
  return h("div", { class: "option-row" }, h("span", { id, class: "option-label" }, labelText), control, extra ?? null);
}

// The third choice "전환" sets toggle: true and leaves the on/off field at its last value.
const TOGGLE = { value: "toggle", label: "전환" };
const BOOL_FORMS = {
  speaker_mute: { field: "mute", label: "스피커", options: [{ value: true, label: "음소거" }, { value: false, label: "음소거 해제" }, TOGGLE] },
  mic_mute: { field: "mute", label: "마이크", options: [{ value: true, label: "음소거" }, { value: false, label: "음소거 해제" }, TOGGLE] },
  desktop_icons: { field: "hide", label: "바탕 화면 아이콘", options: [{ value: true, label: "숨기기" }, { value: false, label: "표시" }, TOGGLE] },
  hidden_items: { field: "show", label: "숨긴 항목", options: [{ value: true, label: "표시" }, { value: false, label: "숨기기" }, TOGGLE] },
  theme: { field: "dark", label: "Windows 테마", options: [{ value: true, label: "어둡게" }, { value: false, label: "밝게" }, TOGGLE] },
};

const FORMS = {
  bool(step) {
    const spec = BOOL_FORMS[step.kind];
    const seg = segmented(spec.options, step.toggle ? TOGGLE.value : step[spec.field], null, spec.label);
    return {
      body: h("div", {}, optionRow(spec.label, seg),
        h("p", { class: "note" }, "전환: 지금과 반대로")),
      read: () => {
        const v = seg.value();
        return { step: v === TOGGLE.value ? { ...step, toggle: true } : { ...step, [spec.field]: v, toggle: false } };
      },
    };
  },

  volume(step) {
    const out = h("output", { class: "range-value" }, `${step.percent}%`);
    const range = h("input", { type: "range", min: "0", max: "100", step: "1", value: String(step.percent), "aria-label": "음량",
      oninput: () => { out.textContent = `${range.value}%`; } });
    return {
      body: h("div", {}, h("div", { class: "field" }, h("span", { class: "option-label" }, "음량"), h("div", { class: "range-row" }, range, out)),
        h("p", { class: "note" }, "기본 스피커에 적용")),
      read: () => ({ step: { ...step, percent: Number(range.value) } }),
    };
  },

  wait(step) {
    const input = h("input", { type: "number", min: "0.1", max: "60", step: "0.1", value: String(step.ms / 1000), autofocus: true });
    return {
      body: field("기다릴 시간 (초)", input, "0.1~60초"),
      ready: () => input.value.trim() !== "",
      read: () => {
        const sec = Number(input.value);
        if (!Number.isFinite(sec) || sec < 0.1 || sec > 60) return { error: "0.1초에서 60초 사이로 입력하세요.", focus: input };
        return { step: { ...step, ms: Math.round(sec * 1000) } };
      },
    };
  },

  text(step) {
    const area = h("textarea", { rows: "4", maxlength: "2000", autofocus: true });
    area.value = step.text;
    return {
      body: field("입력할 글자", area, "커서 위치에 입력"),
      ready: () => area.value !== "",
      read: () => (area.value ? { step: { ...step, text: area.value } } : { error: "입력할 글자를 적으세요.", focus: area }),
    };
  },

  keys(step) {
    // Win combinations are often taken by Windows before the page sees them,
    // so modifiers can also be toggled by hand and only the main key recorded.
    const parsed = (step.spec || "").split("+").filter(Boolean);
    const MODS = [["Control", "Ctrl"], ["Alt", "Alt"], ["Shift", "Shift"], ["Super", "Win"]];
    const mods = new Set(parsed.filter((p) => MODS.some(([m]) => m === p)));
    let key = parsed.find((p) => !MODS.some(([m]) => m === p)) ?? "";
    let recording = false;

    const preview = h("span", { class: "keys-preview" });
    const modButtons = MODS.map(([m, text]) => h("button", { type: "button", class: "toggle", "aria-pressed": "false",
      onclick: () => { mods.has(m) ? mods.delete(m) : mods.add(m); paint(); } }, text));
    const recordBtn = h("button", { type: "button", class: "chip", autofocus: true, onclick: () => (recording ? stop() : start()) });

    function spec() { return key ? [...MODS.map(([m]) => m).filter((m) => mods.has(m)), key].join("+") : ""; }
    function paint() {
      modButtons.forEach((b, n) => b.setAttribute("aria-pressed", mods.has(MODS[n][0]) ? "true" : "false"));
      recordBtn.classList.toggle("recording", recording);
      recordBtn.textContent = recording ? "키를 누르세요… Esc 취소" : key ? `키: ${specLabel(key)} · 다시 녹음` : "눌러서 키 녹음";
      preview.textContent = spec() ? specLabel(spec()) : "아직 키가 없습니다";
    }
    function onKey(e) {
      if (!recording) return;
      if (e.key === "Tab" && !e.ctrlKey && !e.altKey && !e.metaKey) return stop(); // let Tab move on
      e.preventDefault();
      e.stopImmediatePropagation();
      if (e.code === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) return stop();
      const k = specFromEvent(e);
      if (!k) return;
      key = e.code;
      // Held modifiers replace the toggles; a bare key keeps what was toggled on by hand.
      if (k.mods.length) {
        for (const [m] of MODS) mods.delete(m);
        k.mods.forEach((m) => mods.add(m));
      }
      stop();
    }
    async function start() {
      recording = true;
      paint();
      window.addEventListener("keydown", onKey, true);
      try { await invoke("begin_hotkey_capture"); } catch {}
    }
    async function stop() {
      if (!recording) return;
      recording = false;
      window.removeEventListener("keydown", onKey, true);
      paint();
      try { render(await invoke("end_hotkey_capture")); } catch {}
    }
    paint();
    return {
      body: h("div", {},
        h("div", { class: "field" }, h("span", { class: "option-label" }, "보낼 단축키"),
          h("div", { class: "keys-row" }, recordBtn), h("div", { class: "keys-mods", role: "group", "aria-label": "보조 키" }, modButtons)),
        h("p", { class: "keys-result" }, "보낼 키: ", preview),
        h("p", { class: "note" }, "Win 조합이 녹음되지 않으면 Win을 켠 뒤 나머지 키만 녹음하세요.")),
      ready: () => !!key,
      read: () => (key ? { step: { ...step, spec: spec() } } : { error: "보낼 키를 녹음하세요.", focus: recordBtn }),
      dispose: stop,
    };
  },

  path(step) {
    const input = h("input", { type: "text", value: step.path, placeholder: "예: D:\\문서\\보고서.docx", autofocus: true, spellcheck: "false" });
    const pick = async (folder) => {
      try {
        const p = await invoke("pick_path", { folder });
        if (p) setPicked(input, p);
      } catch (err) { input.setCustomValidity(String(err)); }
      input.focus();
    };
    return {
      body: h("div", {},
        field("열 파일 또는 폴더", input),
        h("div", { class: "button-row" },
          h("button", { type: "button", class: "button", onclick: () => pick(false) }, "파일 선택…"),
          h("button", { type: "button", class: "button", onclick: () => pick(true) }, "폴더 선택…"))),
      ready: () => input.value.trim() !== "",
      read: () => (input.value.trim() ? { step: { ...step, path: input.value.trim() } } : { error: "열 파일이나 폴더를 지정하세요.", focus: input }),
    };
  },

  url(step) {
    const input = h("input", { type: "url", value: step.url, placeholder: "https://", autofocus: true, spellcheck: "false" });
    let browser = { ...step.browser };
    const newWin = h("input", { type: "checkbox", checked: step.new_window });
    const priv = h("input", { type: "checkbox", checked: step.private });
    const privLabel = h("label", {}, priv, h("span", {}));
    const paintPrivate = () => {
      privLabel.lastChild.textContent = browser.browser === "edge" ? "InPrivate 창으로 열기" : "시크릿 창으로 열기";
    };
    const list = h("div", { class: "radio-list", role: "radiogroup", "aria-label": "어디서 열까요" },
      h("p", { class: "sub loading" }, "브라우저를 찾는 중…"));

    const options = [{ value: "default|", label: "기본 브라우저", browser: { browser: "default", profile_dir: "", profile_name: "" } }];
    const draw = () => {
      const current = `${browser.browser}|${browser.browser === "default" ? "" : browser.profile_dir}`;
      if (!options.some((o) => o.value === current)) {
        options.push({ value: current, label: `${browserText(browser)} (찾을 수 없음)`, browser });
      }
      list.replaceChildren(...options.map((o, i) => {
        const on = o.value === current;
        return h("button", { type: "button", role: "radio", class: "radio", "aria-checked": on ? "true" : "false", tabindex: on ? "0" : "-1",
          onclick: () => { browser = { ...o.browser }; draw(); paintPrivate(); list.querySelector('[aria-checked="true"]').focus(); },
          onkeydown: (e) => {
            const d = { ArrowDown: 1, ArrowUp: -1 }[e.key];
            if (!d) return;
            e.preventDefault();
            browser = { ...options[(i + d + options.length) % options.length].browser };
            draw(); paintPrivate();
            list.querySelector('[aria-checked="true"]').focus();
          } },
        h("span", { class: "dot", "aria-hidden": "true" }), h("span", {}, o.label));
      }));
    };
    invoke("list_browsers").then((profiles) => {
      for (const pr of profiles) {
        const name = pr.browser === "edge" ? "Edge" : "Chrome";
        options.push({ value: `${pr.browser}|${pr.dir}`, label: `${name} · ${pr.name}`, browser: { browser: pr.browser, profile_dir: pr.dir, profile_name: pr.name } });
      }
      draw();
    }).catch(() => draw());
    paintPrivate();

    return {
      body: h("div", {},
        field("주소", input),
        h("div", { class: "field" }, h("span", { class: "option-label" }, "어디서 열까요"), list,
          h("span", { class: "sub" }, "프로필 이름만 읽습니다.")),
        h("div", { class: "checks" }, h("label", {}, newWin, h("span", {}, "새 창으로 열기")), privLabel)),
      ready: () => input.value.trim() !== "",
      read: () => {
        let v = input.value.trim();
        if (v && !/^[a-z][a-z0-9+.-]*:/i.test(v)) v = `https://${v}`;
        let ok = false;
        try { const u = new URL(v); ok = (u.protocol === "http:" || u.protocol === "https:") && !!u.host; } catch {}
        if (!ok) return { error: "http:// 또는 https://로 시작하는 웹 주소를 입력하세요.", focus: input };
        return { step: { ...step, url: v, browser, new_window: newWin.checked, private: priv.checked } };
      },
    };
  },

  app(step) {
    const chooser = appChooser(step.app);
    const ifRunning = comboField(IF_RUNNING, step.if_running ?? "front", () => {});
    const monitorOpts = [
      { value: "app_default", label: "앱 기본 위치" }, { value: "primary", label: "주 모니터" }, { value: "cursor", label: "마우스가 있는 모니터" },
    ];
    const monitorKey = (m) => (m?.kind === "specific" ? `m:${m.id}` : m?.kind ?? "app_default");
    const monitorNames = {};
    const monitor = comboField(monitorOpts, monitorKey(step.monitor), () => {});
    if (step.monitor?.kind === "specific") {
      monitorNames[step.monitor.id] = step.monitor.name;
      monitor.combo.options.push({ value: monitorKey(step.monitor), label: `${step.monitor.name} (연결 안 됨)` });
      monitor.combo.setValue(monitorKey(step.monitor));
    }
    invoke("list_monitors").then((list) => {
      for (const m of list) {
        const value = `m:${m.id}`;
        monitorNames[m.id] = m.name;
        const text = m.primary ? `${m.name} (주 모니터)` : m.name;
        const existing = monitor.combo.options.find((o) => o.value === value);
        if (existing) existing.label = text;
        else monitor.combo.options.push({ value, label: text });
      }
      monitor.combo.setValue(monitor.combo.value);
    }).catch(() => {});
    const windowState = comboField(WINDOW_STATES, step.window ?? "app_default", () => {});
    const closeNote = h("span", { class: "note warn", hidden: step.on_restore !== "close" }, CLOSE_NOTE);
    const onRestore = comboField(ON_RESTORE, step.on_restore ?? "keep", (v) => { closeNote.hidden = v !== "close"; });

    return {
      body: h("div", {}, chooser.el,
        h("div", { class: "options card" },
          optionRow("이미 실행 중이면", ifRunning.el),
          optionRow("모니터", monitor.el),
          optionRow("창 상태", windowState.el),
          optionRow("원래대로 할 때 이 앱은", onRestore.el, closeNote)),
        h("p", { class: "note" }, "관리자 권한 앱은 옮기지 못할 수 있습니다.")),
      ready: () => chooser.ready(),
      read: () => {
        const app = chooser.read();
        if (app.error) return app;
        const mv = monitor.combo.value;
        const mon = mv.startsWith("m:") ? { kind: "specific", id: mv.slice(2), name: monitorNames[mv.slice(2)] ?? "" } : { kind: mv };
        return { step: { ...step, app: app.app, if_running: ifRunning.combo.value, monitor: mon, window: windowState.combo.value, on_restore: onRestore.combo.value } };
      },
    };
  },

  app_windows(step) {
    const chooser = appChooser(step.app);
    const note = h("p", { class: "note warn", hidden: step.op !== "close" }, CLOSE_NOTE);
    const seg = segmented([{ value: "minimize", label: "최소화" }, { value: "close", label: "닫기" }], step.op,
      (v) => { note.hidden = v !== "close"; }, "할 일");
    return {
      body: h("div", {}, chooser.el, optionRow("할 일", seg), note,
        h("p", { class: "note" }, "그 앱의 모든 창")),
      ready: () => chooser.ready(),
      read: () => {
        const app = chooser.read();
        if (app.error) return app;
        return { step: { ...step, app: app.app, op: seg.value() } };
      },
    };
  },

  folder_tree(step) {
    const input = h("input", { type: "text", value: step.path ?? "", placeholder: "예: D:\\사진\\2024", autofocus: true, spellcheck: "false" });
    const browse = h("button", { type: "button", class: "button", onclick: async () => {
      try { const p = await invoke("pick_path", { folder: true }); if (p) setPicked(input, p); } catch {}
      input.focus();
    } }, "찾아보기…");
    const pathPanel = h("div", { class: "chooser-panel" },
      h("div", { class: "field" }, h("label", { for: input.id = nextId("folder") }, "폴더"),
        h("div", { class: "input-row" }, input, browse)));
    // selected: true = the folder selected in File Explorer when the shortcut is pressed (path ignored).
    let selected = step.selected === true;
    const target = segmented([{ value: true, label: "탐색기에서 선택한 폴더" }, { value: false, label: "지정한 폴더" }], selected,
      (v) => { selected = v; pathPanel.hidden = v; }, "대상");
    pathPanel.hidden = selected;
    const seg = segmented([{ value: true, label: "숨기기" }, { value: false, label: "숨김 해제" }], step.hide !== false, null, "할 일");
    const self = h("input", { type: "checkbox", checked: step.include_self !== false });
    return {
      body: h("div", {},
        h("div", { class: "field" }, h("span", { class: "option-label", "aria-hidden": "true" }, "대상"),
          h("div", { class: "chooser" }, target, pathPanel)),
        optionRow("할 일", seg),
        h("div", { class: "checks" }, h("label", {}, self, h("span", {}, "폴더 자체도 포함"))),
        h("p", { class: "note" }, "다시 눌러도 원래대로 안 됨 · 10분 동안 트레이 메뉴에서 실행 취소")),
      ready: () => selected || input.value.trim() !== "",
      read: () => {
        if (selected) return { step: { ...step, selected: true, hide: seg.value(), include_self: self.checked } };
        const p = input.value.trim().replace(/^"(.*)"$/, "$1");
        if (!p) return { error: "폴더를 지정하세요.", focus: input };
        // A full path: C:\… or \\server\share\…
        if (!/^([a-z]:[\\/]|\\\\[^\\]+\\[^\\]+)/i.test(p)) return { error: "C:\\…처럼 전체 경로를 입력하거나 ‘찾아보기…’로 선택하세요.", focus: input };
        return { step: { ...step, selected: false, path: p, hide: seg.value(), include_self: self.checked } };
      },
    };
  },
};

// ---------- app chooser (installed apps or a file) ----------

let appsCache = null;
const loadApps = () => (appsCache ??= invoke("list_apps").catch((err) => { appsCache = null; throw err; }));

const TILE_COLORS = ["#3A6EA5", "#2E7D5B", "#6B5BA8", "#B4512F", "#2F6FB3", "#8A5A2B", "#A23B72", "#3D7A7A"];
function tile(name) {
  let n = 0;
  for (const ch of name) n = (n * 31 + ch.codePointAt(0)) >>> 0;
  return h("span", { class: "app-tile", "aria-hidden": "true", style: `background:${TILE_COLORS[n % TILE_COLORS.length]}` }, [...name][0]?.toUpperCase() ?? "?");
}

function appChooser(app) {
  let mode = app?.source === "file" ? "file" : "installed";
  let selected = app?.source === "installed" ? { id: app.id, name: app.name } : null;

  // installed
  const search = h("input", { type: "search", placeholder: "앱 이름 검색", "aria-label": "앱 검색", autofocus: true,
    oninput: () => drawList(), onkeydown: (e) => { if (e.key === "ArrowDown") { e.preventDefault(); listEl.querySelector('[role="option"]')?.focus(); } } });
  const listEl = h("ul", { class: "app-list", role: "listbox", "aria-label": "설치된 앱", "aria-busy": "true" },
    h("li", { class: "sub loading" }, h("span", { class: "spinner", "aria-hidden": "true" }), "앱 목록을 불러오는 중…"));
  let apps = null;

  function drawList() {
    if (!apps) return;
    const q = search.value.trim().toLowerCase();
    const shown = apps.filter((a) => a.name.toLowerCase().includes(q));
    if (selected && !apps.some((a) => a.id === selected.id) && (!q || selected.name.toLowerCase().includes(q))) {
      shown.unshift({ id: selected.id, name: selected.name, kind: "missing" });
    }
    listEl.replaceChildren(...(shown.length ? shown.slice(0, 300).map((a, i) => {
      const on = selected?.id === a.id;
      return h("li", { role: "option", tabindex: on || (!selected && i === 0) ? "0" : "-1", "aria-selected": on ? "true" : "false",
        onclick: () => { selected = { id: a.id, name: a.name }; drawList(); listEl.querySelector('[aria-selected="true"]')?.focus(); },
        onkeydown: (e) => {
          const items = [...listEl.querySelectorAll('[role="option"]')];
          const idx = items.indexOf(e.currentTarget);
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            items[Math.max(0, Math.min(items.length - 1, idx + (e.key === "ArrowDown" ? 1 : -1)))]?.focus();
          } else if (e.key === " " || e.key === "Enter") {
            e.preventDefault();
            e.stopPropagation();
            e.currentTarget.click();
          }
        } },
      a.kind === "missing" ? tile(a.name) : fileIcon(`app:${a.id}`, 24, () => tile(a.name)),
      h("span", { class: "app-name", title: a.name }, a.name),
      h("span", { class: "sub" }, a.kind === "store" ? "Microsoft Store 앱" : a.kind === "missing" ? "찾을 수 없음" : "설치된 프로그램"));
    }) : [h("li", { class: "sub loading" }, "검색 결과가 없습니다.")]));
  }
  loadApps().then((list) => { apps = list; listEl.removeAttribute("aria-busy"); drawList(); })
    .catch((err) => listEl.replaceChildren(h("li", { class: "sub loading" }, `앱 목록을 읽지 못했습니다. ${err}`)));

  const installedPanel = h("div", { class: "chooser-panel" },
    h("div", { class: "search" }, icon("search"), search), listEl);

  // file
  const pathInput = h("input", { type: "text", value: app?.source === "file" ? app.id : "", placeholder: "C:\\Program Files\\…\\app.exe", spellcheck: "false" });
  const argsInput = h("input", { type: "text", value: app?.source === "file" ? app.args ?? "" : "", spellcheck: "false" });
  const browse = h("button", { type: "button", class: "button", onclick: async () => {
    try { const p = await invoke("pick_file"); if (p) setPicked(pathInput, p); } catch {}
    pathInput.focus();
  } }, "찾아보기…");
  const filePanel = h("div", { class: "chooser-panel" },
    h("div", { class: "field" }, h("label", { for: pathInput.id = nextId("path") }, "프로그램(.exe) 또는 바로 가기(.lnk)"),
      h("div", { class: "input-row" }, pathInput, browse)),
    field("프로그램에 넘길 값 (선택)", argsInput, "예: 열 문서 경로"));

  const seg = segmented([{ value: "installed", label: "설치된 앱" }, { value: "file", label: "파일에서 직접 지정" }], mode,
    (v) => { mode = v; installedPanel.hidden = v !== "installed"; filePanel.hidden = v !== "file"; }, "앱을 고르는 방법");
  installedPanel.hidden = mode !== "installed";
  filePanel.hidden = mode !== "file";

  return {
    el: h("div", { class: "chooser" }, seg, installedPanel, filePanel),
    ready: () => (mode === "installed" ? !!selected : pathInput.value.trim() !== ""),
    read: () => {
      if (mode === "installed") {
        if (!selected) return { error: "목록에서 앱을 고르세요.", focus: search };
        return { app: { source: "installed", id: selected.id, name: selected.name, args: "" } };
      }
      const p = pathInput.value.trim().replace(/^"(.*)"$/, "$1");
      if (!/\.(exe|lnk)$/i.test(p)) return { error: "프로그램(.exe)이나 바로 가기(.lnk)를 지정하세요.", focus: pathInput };
      return { app: { source: "file", id: p, name: baseName(p).replace(/\.(exe|lnk)$/i, ""), args: argsInput.value.trim() } };
    },
  };
}

// ---------- wiring ----------

$("preset-new").addEventListener("click", createPreset);
window.addEventListener("keyscene:view", () => { resetTransient(); renderList(); });
// Closing or hiding the window does not blur the name field, so save a name still on its timer.
window.addEventListener("blur", flushNames);
window.addEventListener("pagehide", flushNames);

window.addEventListener("keyscene:hotkeys", (e) => {
  const action = e.detail?.action ?? "";
  if (action.startsWith("preset:")) {
    const id = action.slice(7);
    if (e.detail.error) chipErrors[id] = e.detail.error;
    else delete chipErrors[id];
  }
  loadPresets();
});

listen("presets", (e) => applyData(e.payload));
$("preset-list").replaceChildren(h("p", { class: "empty-note loading", role: "status" },
  h("span", { class: "spinner", "aria-hidden": "true" }), "단축키를 불러오는 중…"));
loadPresets();

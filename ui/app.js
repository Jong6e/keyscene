// Settings window shell: the two views (단축키 list / 설정), the 설정 view's rows, the update check,
// and shortcut recording for the 단축키 cards. Uses components.js (loaded first).

let state = null;
let recordingAction = null; // action id ("preset:<id>") whose shortcut is being recorded
let pendingWarn = null;     // { action, spec } waiting for 그래도 지정 / 취소

// ---------- views: 단축키 list (always first) and 설정 ----------

let view = "list";
let listScroll = 0; // the list's scroll position, put back when coming back from 설정

$("preset-new").append(icon("plus"));
$("open-settings").append(icon("gear"));
$("settings-back").append(icon("arrowLeft"));

/// Where focus goes when nothing better is left (e.g. a toast that had focus closes).
function homeFocus() {
  return view === "settings" ? $("settings-back") : $("preset-new");
}

function showView(name) {
  if (name === view) return;
  const scroller = document.scrollingElement;
  if (name === "settings") listScroll = scroller.scrollTop;
  view = name;
  clearWarning(false);
  // Lets presets.js drop half-finished edits (open lists, delete confirmations, recording).
  window.dispatchEvent(new CustomEvent("keyscene:view", { detail: name }));
  const settings = name === "settings";
  $("view-list").hidden = settings;
  $("view-settings").hidden = !settings;
  $("view-title").textContent = settings ? "설정" : "단축키";
  $("settings-back").hidden = !settings;
  $("preset-new").hidden = settings;
  $("open-settings").hidden = settings;
  scroller.scrollTo({ top: settings ? 0 : listScroll, behavior: "instant" });
  (settings ? $("settings-back") : $("open-settings")).focus();
}

$("open-settings").addEventListener("click", () => showView("settings"));
$("settings-back").addEventListener("click", () => showView("list"));
// Esc (or Alt+←, like Windows Settings) goes back. Dialogs, open lists, the shortcut warning and
// shortcut recording handle Esc themselves first.
window.addEventListener("keydown", (e) => {
  if (view !== "settings" || e.defaultPrevented || Dialog.el || recordingAction) return;
  const back = (e.key === "Escape" && !e.altKey && !e.ctrlKey && !e.shiftKey && !e.metaKey)
    || (e.key === "ArrowLeft" && e.altKey && !e.ctrlKey && !e.shiftKey && !e.metaKey);
  if (!back) return;
  e.preventDefault();
  showView("list");
});

// ---------- 설정 view ----------

function setSwitch(el, on) {
  el.setAttribute("aria-checked", on ? "true" : "false");
}

/// Shows a shortcut on a chip (unless that chip is recording right now).
function paintChip(chip, spec, label) {
  if (!chip || chip.dataset.action === recordingAction) return;
  chip.classList.remove("recording");
  chip.classList.toggle("empty", !spec);
  chip.textContent = spec ? label.replaceAll("+", " + ") : "단축키 지정";
  chip.setAttribute("aria-label", spec ? `단축키 ${label}, 눌러서 변경` : "단축키 지정");
}

/// Draws a StateDto: { autostart, paused, notice, excluded_apps, suspended_by }.
function render(s) {
  state = s;
  setSwitch($("autostart"), s.autostart);
  setSwitch($("paused"), s.paused);
  $("notice").hidden = !s.notice;
  $("notice-text").textContent = s.notice ?? "";
  if (typeof renderExcluded === "function") renderExcluded(s);
  placeWarning();
}

/// The line under a row. `hint` = neutral guidance (grey) instead of an error (red).
function showHelp(action, text, hint = false) {
  const help = document.querySelector(`[data-help="${CSS.escape(action)}"]`);
  if (!help) return;
  help.hidden = !text;
  // Errors are announced at once; guidance politely.
  help.setAttribute("role", hint ? "status" : "alert");
  help.textContent = text ?? "";
  help.classList.toggle("hint", !!text && hint);
}

const RECORD_HINT = "예: Ctrl+Alt+H · Esc 취소 · Backspace 지우기";

/// Runs a command; on failure shows the message under the related row (or the notice bar).
/// Returns the error message, or null on success.
async function call(cmd, args, action) {
  try {
    render(await invoke(cmd, args));
    return null;
  } catch (err) {
    render(await invoke("get_state"));
    if (action) showHelp(action, String(err));
    else {
      $("notice").hidden = false;
      $("notice-text").textContent = String(err);
    }
    return String(err);
  }
}

$("autostart").addEventListener("click", () => call("set_autostart", { enabled: !state.autostart }));
$("paused").addEventListener("click", () => call("set_paused", { paused: !state.paused }));
$("notice-close").addEventListener("click", async () => {
  await invoke("dismiss_notice");
  render(await invoke("get_state"));
});

// ---------- 정보: version and "새 버전 확인" ----------

// The version comes from tauri.conf.json, so it is never out of date here.
const versionReady = window.__TAURI__.app.getVersion().then((v) => {
  $("app-version").textContent = `Keyscene ${v}`;
  return v;
}, () => null);

const RELEASE_API = "https://api.github.com/repos/Jong6e/keyscene/releases/latest";

/// "v0.3.1" / "0.3.1-beta" -> [0, 3, 1]; null when it does not start with a number.
function parseVersion(text) {
  const m = /^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?/i.exec(String(text ?? "").trim());
  return m ? [m[1], m[2] ?? 0, m[3] ?? 0].map(Number) : null;
}

function isNewer(a, b) {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i];
  return false;
}

/// result: null (clear) | { text, download?, busy? }
function showUpdate(result) {
  const text = $("update-text");
  text.replaceChildren(...(result?.busy ? [h("span", { class: "spinner", "aria-hidden": "true" })] : []), result?.text ?? "");
  $("update-actions").hidden = !result?.download;
}

const UPDATE_FAILED = "확인하지 못했습니다. 인터넷 연결을 확인하고 다시 시도하세요.";
let checkingUpdate = false;

/// The only place the app goes on the internet, and only when the button is pressed.
async function checkForUpdate() {
  if (checkingUpdate) return; // the button stays enabled (keeps focus); a second press waits
  checkingUpdate = true;
  $("update-check").setAttribute("aria-disabled", "true");
  showUpdate({ text: "확인하는 중…", busy: true });
  let result;
  try {
    const current = parseVersion(await versionReady);
    const stop = new AbortController();
    const timer = setTimeout(() => stop.abort(), 10000);
    let res;
    try {
      res = await fetch(RELEASE_API, {
        headers: { Accept: "application/vnd.github+json" },
        cache: "no-store", credentials: "omit", referrerPolicy: "no-referrer", signal: stop.signal,
      });
    } finally {
      clearTimeout(timer);
    }
    if (!res.ok) throw new Error(`HTTP ${res.status}`); // 404 = no release yet, 403 = rate limit
    const tag = String((await res.json())?.tag_name ?? "").trim().replace(/^v/i, "");
    const latest = parseVersion(tag);
    if (!current || !latest) throw new Error("version");
    result = isNewer(latest, current) ? { text: `새 버전 ${tag}가 있습니다.`, download: true } : { text: "최신 버전입니다." };
  } catch {
    result = { text: UPDATE_FAILED };
  }
  checkingUpdate = false;
  $("update-check").removeAttribute("aria-disabled");
  showUpdate(result);
}

$("update-check").addEventListener("click", checkForUpdate);
$("update-open").addEventListener("click", async () => {
  try {
    await invoke("open_release_page");
  } catch {
    showUpdate({ text: "다운로드 페이지를 열지 못했습니다. 잠시 뒤 다시 시도하세요.", download: true });
  }
});

// ---------- shortcut recording (단축키 card chips) ----------

/// Cards are redrawn after a shortcut change; `error` explains a refused shortcut.
const hotkeyChanged = (action, error = null) =>
  window.dispatchEvent(new CustomEvent("keyscene:hotkeys", { detail: { action, error } }));

function chipFor(action) {
  return document.querySelector(`.chip[data-action="${CSS.escape(action)}"]`);
}

async function startRecording(chip) {
  if (recordingAction) await stopRecording();
  clearWarning(false);
  recordingAction = chip.dataset.action;
  chip.classList.add("recording");
  chip.classList.remove("empty");
  chip.textContent = "키를 누르세요…";
  chip.setAttribute("aria-label", `단축키 입력 대기 중. ${RECORD_HINT}`);
  showHelp(recordingAction, RECORD_HINT, true);
  await invoke("begin_hotkey_capture");
}

async function stopRecording() {
  const action = recordingAction;
  recordingAction = null;
  showHelp(action, null);
  render(await invoke("end_hotkey_capture"));
  hotkeyChanged(action);
}

// Delegated, so chips of cards drawn later work too.
document.addEventListener("click", (e) => {
  const chip = e.target.closest?.(".chip[data-action]");
  if (chip) startRecording(chip);
});
document.addEventListener("focusout", (e) => {
  const chip = e.target.closest?.(".chip[data-action]");
  if (chip && recordingAction === chip.dataset.action) stopRecording();
});

window.addEventListener("keydown", async (e) => {
  if (!recordingAction) return;
  const action = recordingAction;
  // Tab is never captured, so keyboard users can always leave the chip.
  if (e.key === "Tab" && !e.ctrlKey && !e.altKey && !e.metaKey) return stopRecording();
  e.preventDefault();
  e.stopPropagation();
  if (e.code === "Escape") return stopRecording();
  if (e.code === "Backspace" || e.code === "Delete") {
    recordingAction = null;
    return hotkeyChanged(action, await call("set_hotkey", { action, spec: null }, action));
  }
  const key = specFromEvent(e);
  if (!key) return;
  const isFunctionKey = /^F([1-9]|1[0-9]|2[0-4])$/.test(e.code);
  if (key.mods.length === 0 && !isFunctionKey) {
    const chip = chipFor(action);
    if (chip) chip.textContent = "Ctrl, Alt, Shift, Win 중 하나와 함께";
    return;
  }
  recordingAction = null;
  if (isCommonShortcut(key.spec)) {
    // Ask first: taking Ctrl+C or Alt+F4 away from every app is rarely what people want.
    showHelp(action, null);
    render(await invoke("end_hotkey_capture"));
    hotkeyChanged(action);
    setWarning(action, key.spec);
    return;
  }
  hotkeyChanged(action, await call("set_hotkey", { action, spec: key.spec }, action));
}, true);

// ---------- warning before taking a widely used shortcut ----------

function setWarning(action, spec) {
  pendingWarn = { action, spec };
  placeWarning();
  document.querySelector(`[data-key="warn-cancel:${CSS.escape(action)}"]`)?.focus();
}

function clearWarning(refocus) {
  const action = pendingWarn?.action;
  pendingWarn = null;
  document.querySelectorAll(".hotkey-warning").forEach((e) => e.remove());
  if (refocus && action) chipFor(action)?.focus();
}

async function confirmWarning() {
  const { action, spec } = pendingWarn;
  clearWarning(false);
  hotkeyChanged(action, await call("set_hotkey", { action, spec }, action));
  chipFor(action)?.focus();
}

/// Puts the warning right under its chip's row; called after every redraw (rows get rebuilt).
function placeWarning() {
  for (const e of document.querySelectorAll(".hotkey-warning")) {
    if (!pendingWarn || e.dataset.warn !== pendingWarn.action) e.remove();
  }
  if (!pendingWarn) return;
  const help = document.querySelector(`[data-help="${CSS.escape(pendingWarn.action)}"]`);
  if (!help || help.nextElementSibling?.classList.contains("hotkey-warning")) return;
  const { action, spec } = pendingWarn;
  help.after(h("div", { class: "hotkey-warning", role: "alert", "data-warn": action,
    onkeydown: (e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); clearWarning(true); } } },
    h("span", { class: "warn-text" }, `${specLabel(spec)}: 많은 앱이 쓰는 키입니다. 지정하면 다른 앱에서는 동작하지 않습니다.`),
    h("span", { class: "warn-actions" },
      h("button", { type: "button", class: "button primary", "data-key": `warn-ok:${action}`, onclick: confirmWarning }, "그래도 지정"),
      h("button", { type: "button", class: "button", "data-key": `warn-cancel:${action}`, onclick: () => clearWarning(true) }, "취소"))));
}

listen("state", (e) => { if (!recordingAction) render(e.payload); });
invoke("get_state").then((s) => {
  render(s);
  requestAnimationFrame(() => invoke("ui_ready"));
});

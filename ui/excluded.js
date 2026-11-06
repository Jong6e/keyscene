// 설정 view: apps that pause Keyscene's shortcuts while they are in front ("단축키를 쓰지 않을 앱").
// Uses components.js, app.js (state, render, showHelp) and presets.js (tile, baseName).

let excluded = [];

/// Called from app.js render() with every StateDto.
function renderExcluded(s) {
  excluded = s.excluded_apps ?? [];
  const status = $("excluded-status");
  status.hidden = !s.suspended_by;
  status.textContent = s.suspended_by ? `${s.suspended_by}이(가) 앞에 있어 쉬는 중` : "";

  const active = document.activeElement?.dataset?.key;
  const list = $("excluded-list");
  list.replaceChildren(...(excluded.length
    ? excluded.map((a, i) => h("li", { class: "excluded-row" },
      appIcon(a),
      h("div", { class: "label" }, h("span", { class: "app-name", title: a.name || a.exe }, a.name || a.exe), h("span", { class: "sub", title: a.path || a.exe }, a.exe)),
      h("button", { type: "button", class: "icon-button small", "data-key": `excluded-remove:${i}`,
        "aria-label": `${a.name || a.exe} 목록에서 제거`, title: "목록에서 제거",
        // Ignore the 2nd click of a double-click: the next row has moved under the pointer.
        onclick: (e) => { if (e.detail <= 1) removeExcluded(i); } }, icon("close"))))
    : [h("li", { class: "excluded-empty sub" }, "없음")]));
  if (active) list.querySelector(`[data-key="${CSS.escape(active)}"]`)?.focus({ preventScroll: true });
}

/// The program's own icon when its full path is known, else the letter tile.
const appIcon = (a) => fileIcon(a.path ? `path:${a.path}` : null, 24, () => tile(a.name || a.exe));

/// Saves the whole list. Returns the error message, or null.
async function saveExcluded(apps) {
  try {
    render(await invoke("set_excluded_apps", { apps }));
    showHelp("excluded", null);
    return null;
  } catch (err) {
    return String(err);
  }
}

/// One click removes; the toast offers 실행 취소 (or Ctrl+Z) to put the app back where it was.
async function removeExcluded(i) {
  const removed = excluded[i];
  const next = excluded.filter((_, n) => n !== i);
  const err = await saveExcluded(next);
  if (err) return showHelp("excluded", err);
  // Focus the row that moved up, or "앱 추가" when the list is empty.
  const target = $("excluded-list").querySelector(`[data-key="excluded-remove:${Math.min(i, next.length - 1)}"]`) ?? $("excluded-add");
  target.focus();
  Toast.show(`‘${removed.name || removed.exe}’을(를) 목록에서 제거했습니다.`, {
    focusAfter: () => $("excluded-add"),
    undo: async () => {
      if (excluded.some((a) => a.exe === removed.exe)) return;
      const back = [...excluded];
      back.splice(Math.min(i, back.length), 0, removed);
      const e2 = await saveExcluded(back);
      if (e2) return showHelp("excluded", e2);
      $("excluded-list").querySelector(`[data-key="excluded-remove:${Math.min(i, back.length - 1)}"]`)?.focus();
    },
  });
}

// ---------- "앱 추가" dialog ----------

function openExcludedDialog() {
  let mode = "running";
  let selected = null; // { name, exe, path }

  // running apps
  const search = h("input", { type: "search", placeholder: "앱 이름 검색", "aria-label": "실행 중인 앱 검색", autofocus: true,
    oninput: () => draw(),
    onkeydown: (e) => { if (e.key === "ArrowDown") { e.preventDefault(); listEl.querySelector('[role="option"]')?.focus(); } } });
  const listEl = h("ul", { class: "app-list", role: "listbox", "aria-label": "실행 중인 앱", "aria-busy": "true" },
    h("li", { class: "sub loading" }, h("span", { class: "spinner", "aria-hidden": "true" }), "실행 중인 앱을 찾는 중…"));
  let apps = null;
  const taken = new Set(excluded.map((a) => a.exe.toLowerCase()));

  function draw() {
    if (!apps) return;
    if (!apps.length) {
      listEl.replaceChildren(h("li", { class: "sub loading" }, "창이 열려 있는 앱이 없습니다"));
      return;
    }
    const q = search.value.trim().toLowerCase();
    const shown = apps.filter((a) => `${a.name} ${a.exe}`.toLowerCase().includes(q));
    listEl.replaceChildren(...(shown.length ? shown.map((a, i) => {
      const already = taken.has(a.exe.toLowerCase());
      const on = selected?.exe === a.exe;
      return h("li", { role: "option", tabindex: on || (!selected && i === 0) ? "0" : "-1",
        "aria-selected": on ? "true" : "false", "aria-disabled": already ? "true" : null, class: already ? "disabled" : null,
        onclick: () => { if (already) return; selected = a; draw(); listEl.querySelector('[aria-selected="true"]')?.focus(); },
        onkeydown: (e) => {
          const items = [...listEl.querySelectorAll('[role="option"]')];
          const idx = items.indexOf(e.currentTarget);
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            items[Math.max(0, Math.min(items.length - 1, idx + (e.key === "ArrowDown" ? 1 : -1)))]?.focus();
          } else if (e.key === " " || e.key === "Enter") {
            e.preventDefault();
            e.stopPropagation();
            if (on && e.key === "Enter") commit(); else e.currentTarget.click();
          }
        } },
      appIcon(a),
      h("span", { class: "app-name", title: a.name || a.exe }, a.name || a.exe),
      h("span", { class: "sub", title: a.exe }, already ? "이미 추가됨" : a.exe));
    }) : [h("li", { class: "sub loading" }, "검색 결과가 없습니다.")]));
  }
  // Running apps change all the time, so this list is read fresh each time the dialog opens.
  invoke("list_running_apps")
    .then((list) => { apps = list; listEl.removeAttribute("aria-busy"); draw(); })
    .catch((err) => { listEl.removeAttribute("aria-busy"); listEl.replaceChildren(h("li", { class: "sub loading" }, `실행 중인 앱을 읽지 못했습니다. ${err}`)); });

  const runningPanel = h("div", { class: "chooser-panel" },
    h("div", { class: "search" }, icon("search"), search), listEl,
    h("span", { class: "sub" }, "목록에 없으면 ‘파일에서 직접 지정’"));

  // a file
  const pathInput = h("input", { type: "text", placeholder: "C:\\Program Files\\…\\game.exe", spellcheck: "false" });
  const browse = h("button", { type: "button", class: "button", onclick: async () => {
    try { const p = await invoke("pick_file"); if (p) { pathInput.value = p; error.hidden = true; refresh(); } } catch {}
    pathInput.focus();
  } }, "찾아보기…");
  const filePanel = h("div", { class: "chooser-panel", hidden: true },
    h("div", { class: "field" }, h("label", { for: pathInput.id = nextId("path") }, "프로그램(.exe) 또는 바로 가기(.lnk)"),
      h("div", { class: "input-row" }, pathInput, browse)),
    h("span", { class: "sub" }, "파일 이름이 같으면 어디서 실행해도 적용"));

  const seg = segmented([{ value: "running", label: "실행 중인 앱" }, { value: "file", label: "파일에서 직접 지정" }], mode,
    (v) => { mode = v; runningPanel.hidden = v !== "running"; filePanel.hidden = v !== "file"; error.hidden = true; }, "앱을 고르는 방법");

  const error = h("p", { class: "form-error", role: "alert", hidden: true });
  const fail = (text, focus) => { error.hidden = false; error.textContent = text; focus?.focus(); };

  const ready = () => (mode === "running" ? !!selected : pathInput.value.trim() !== "");
  let saving = false;

  async function commit() {
    if (!ready() || saving) return;
    let app;
    if (mode === "running") {
      if (!selected) return fail("목록에서 앱을 고르세요.", search);
      // The full path lets Keyscene show the program's icon; the backend matches by file name either way.
      app = { name: selected.name || selected.exe.replace(/\.exe$/i, ""), exe: selected.path || selected.exe, path: selected.path || null };
    } else {
      const p = pathInput.value.trim().replace(/^"(.*)"$/, "$1");
      // Shortcuts (.lnk) are fine: Keyscene follows them to the program.
      if (!/\.(exe|lnk)$/i.test(p)) return fail("프로그램(.exe)이나 바로 가기(.lnk)를 지정하세요.", pathInput);
      const file = baseName(p);
      app = { name: file.replace(/\.(exe|lnk)$/i, ""), exe: p, path: p };
    }
    const file = baseName(app.exe).toLowerCase();
    if (taken.has(file)) return fail(`${app.name}은(는) 이미 목록에 있습니다.`);
    saving = true;
    add.disabled = true;
    add.textContent = "추가하는 중…";
    const err = await saveExcluded([...excluded, app]);
    saving = false;
    add.textContent = "추가";
    refresh();
    if (err) return fail(err);
    Dialog.close();
  }

  const add = h("button", { type: "button", class: "button primary", onclick: commit }, "추가");
  const body = h("div", { class: "form" }, h("div", { class: "chooser" }, seg, runningPanel, filePanel), error);
  // Windows dialogs: the main button comes first, then 취소.
  Dialog.open({
    title: "단축키를 쓰지 않을 앱 추가",
    wide: true,
    guard: true,
    body,
    footer: [add, h("button", { type: "button", class: "button", onclick: () => Dialog.close() }, "취소")],
  });
  // 추가 is available once an app is chosen; a message clears as soon as something changes.
  const refresh = () => { add.disabled = saving || !ready(); };
  body.addEventListener("input", () => { error.hidden = true; refresh(); });
  body.addEventListener("click", () => refresh());
  refresh();
  pathInput.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); commit(); } });
}

$("excluded-add").addEventListener("click", openExcludedDialog);
if (state) renderExcluded(state);

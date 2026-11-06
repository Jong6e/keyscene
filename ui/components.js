// Shared UI parts: combo box, segmented buttons, modal dialog, shortcut text helpers.
// Classic script (no modules): later scripts use these names directly.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let uid = 0;
const nextId = (prefix) => `${prefix}-${++uid}`;

/// Builds an element: h("div", { class: "x", onclick: fn }, child, "text")
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = v;
    else if (k === "html") el.innerHTML = v; // only for static icon markup
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) {
    if (c == null || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

const ICONS = {
  chevronDown: '<svg viewBox="0 0 14 14" aria-hidden="true"><path d="M3 5l4 4 4-4"/></svg>',
  chevronUp: '<svg viewBox="0 0 14 14" aria-hidden="true"><path d="M3 9l4-4 4 4"/></svg>',
  plus: '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M6 1v10M1 6h10"/></svg>',
  close: '<svg viewBox="0 0 10 10" aria-hidden="true"><path d="M1 1l8 8M9 1l-8 8"/></svg>',
  restore: '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 6a3.5 3.5 0 1 0 1-2.5"/><path d="M2.5 1.5v2h2"/></svg>',
  grip: '<svg viewBox="0 0 12 16" aria-hidden="true"><circle cx="4" cy="4" r="1.2"/><circle cx="8" cy="4" r="1.2"/><circle cx="4" cy="8" r="1.2"/><circle cx="8" cy="8" r="1.2"/><circle cx="4" cy="12" r="1.2"/><circle cx="8" cy="12" r="1.2"/></svg>',
  search: '<svg viewBox="0 0 14 14" aria-hidden="true"><circle cx="6" cy="6" r="4.5"/><path d="M9.5 9.5L13 13"/></svg>',
  back: '<svg viewBox="0 0 14 14" aria-hidden="true"><path d="M9 2.5L4.5 7 9 11.5"/></svg>',
  // Top bar: back arrow (settings view) and the settings gear.
  arrowLeft: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13.5 8h-11M7 3.5L2.5 8 7 12.5"/></svg>',
  gear: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M15.90 7.78L17.96 8.05L17.96 11.95L15.90 12.22L15.74 12.60L17.01 14.25L14.25 17.01L12.60 15.74L12.22 15.90L11.95 17.96L8.05 17.96L7.78 15.90L7.40 15.74L5.75 17.01L2.99 14.25L4.26 12.60L4.10 12.22L2.04 11.95L2.04 8.05L4.10 7.78L4.26 7.40L2.99 5.75L5.75 2.99L7.40 4.26L7.78 4.10L8.05 2.04L11.95 2.04L12.22 4.10L12.60 4.26L14.25 2.99L17.01 5.75L15.74 7.40z"/><circle cx="10" cy="10" r="2.6"/></svg>',
  file: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M5 2.5h6.5l3.5 3.5v11.5H5z"/><path d="M11.5 2.5V6H15"/></svg>',
  folder: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M2.5 5a1 1 0 0 1 1-1h4l1.5 1.5h7.5a1 1 0 0 1 1 1V15a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1z"/></svg>',
  // Step glyphs: every step row starts with a 24px picture, so the text lines up.
  globe: '<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7.5"/><ellipse cx="10" cy="10" rx="3.2" ry="7.5"/><path d="M2.5 10h15"/></svg>',
  window: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="2.5" y="4" width="15" height="12" rx="1.5"/><path d="M2.5 7.5h15"/></svg>',
  desktop: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="2.5" y="3.5" width="15" height="10" rx="1"/><path d="M7 17h6M10 13.5V17"/></svg>',
  speaker: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 8h3l4-3.5v11L6 12H3z"/><path d="M13 7.5a3.5 3.5 0 0 1 0 5M15 5a7 7 0 0 1 0 10"/></svg>',
  mic: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="7.5" y="2.5" width="5" height="9" rx="2.5"/><path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5"/></svg>',
  media: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 5l6 5-6 5z"/><path d="M13.5 5v10M16.5 5v10"/></svg>',
  grid: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="3" width="5.5" height="5.5" rx="1"/><rect x="11.5" y="3" width="5.5" height="5.5" rx="1"/><rect x="3" y="11.5" width="5.5" height="5.5" rx="1"/><rect x="11.5" y="11.5" width="5.5" height="5.5" rx="1"/></svg>',
  eye: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M1.5 10s3-5.5 8.5-5.5S18.5 10 18.5 10s-3 5.5-8.5 5.5S1.5 10 1.5 10z"/><circle cx="10" cy="10" r="2.6"/></svg>',
  theme: '<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7"/><path d="M10 3a7 7 0 0 1 0 14z" fill="currentColor"/></svg>',
  monitorOff: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="2.5" y="3.5" width="15" height="10" rx="1"/><path d="M7 17h6M10 13.5V17M2 2l16 14"/></svg>',
  lock: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="4.5" y="9" width="11" height="8.5" rx="1.5"/><path d="M7 9V6.5a3 3 0 0 1 6 0V9"/></svg>',
  moon: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M15.5 12.5A6.5 6.5 0 0 1 7.5 4.5a6.5 6.5 0 1 0 8 8z"/></svg>',
  clipboard: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="4.5" y="4" width="11" height="13.5" rx="1.5"/><rect x="7.5" y="2.5" width="5" height="3" rx="1"/></svg>',
  capture: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 7V4h3M14 4h3v3M17 13v3h-3M6 16H3v-3"/><circle cx="10" cy="10" r="2.5"/></svg>',
  keyboard: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="2" y="5" width="16" height="10" rx="1.5"/><path d="M5 8.5h1M8.5 8.5h1M12 8.5h1M15 8.5h.5M6 12h8"/></svg>',
  text: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 4.5h12M10 4.5v11M7.5 15.5h5"/></svg>',
  clock: '<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7.5"/><path d="M10 6v4l2.5 2"/></svg>',
  // A dashed selection with an eye: "the selected items' hidden attribute".
  selection: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="2.5" y="3.5" width="15" height="13" rx="1.5" stroke-dasharray="2.2 1.8"/><path d="M5.5 10s1.8-3 4.5-3 4.5 3 4.5 3-1.8 3-4.5 3-4.5-3-4.5-3z"/><circle cx="10" cy="10" r="1.2"/></svg>',
};
const icon = (name, cls = "i") => h("span", { class: cls, html: ICONS[name] });

// ---------- app and file icons (PNG from the backend's "icon" scheme) ----------

/// target: "app:<installed app id>" or "path:<full file/folder/exe path>".
/// null without the Tauri bridge (plain-browser test page).
function iconUrl(target, cssPx) {
  const convert = window.__TAURI__?.core?.convertFileSrc;
  if (!convert) return null;
  return convert(target, "icon") + "?s=" + Math.max(16, Math.round(cssPx * (window.devicePixelRatio || 1)));
}

// Lists are redrawn on every click and keystroke, so remember each answer:
const missingIcons = new Set(); // URLs the backend had no icon for: show the fallback at once
const shownIcons = new Set();   // URLs already loaded: paint them in the same frame (no blink)

/// The real icon of `target` in a fixed size x size box (no layout shift while it loads).
/// fallback() builds the glyph shown when there is no icon. Decorative: the row already has the name.
function fileIcon(target, size, fallback) {
  const url = target ? iconUrl(target, size) : null;
  if (!url || missingIcons.has(url)) return fallback();
  const known = shownIcons.has(url);
  // loading is set before src, so rows scrolled out of view are not fetched (lazy waits for layout,
  // which known icons do not need).
  const img = h("img", { class: "file-icon", alt: "", loading: known ? "eager" : "lazy", decoding: known ? "sync" : "async",
    draggable: "false", width: String(size), height: String(size), src: url });
  img.addEventListener("load", () => shownIcons.add(url), { once: true });
  img.addEventListener("error", () => { missingIcons.add(url); img.replaceWith(fallback()); }, { once: true });
  return img;
}

/// A 24px outline glyph (ICONS.file / ICONS.folder) used where a file has no icon.
const glyph = (name) => h("span", { class: "glyph", "aria-hidden": "true", html: ICONS[name] });

// ---------- Windows 11 combo box ----------

/// A drop-down whose open list we draw ourselves (the built-in one cannot be styled).
/// Only one list is open at a time; global listeners are shared by all instances.
/// While open, the list lives in a fixed full-window layer with overflow hidden, so it can never
/// widen or lengthen the page (no scrollbar appearing, no layout shift).
class Combo {
  static current = null;

  static layer() {
    let el = document.getElementById("popup-layer");
    if (!el) {
      el = h("div", { id: "popup-layer" });
      document.body.append(el);
    }
    return el;
  }

  constructor(root, options, onChange, { formatMissing, labelledBy } = {}) {
    this.root = root;
    this.options = options; // [{ value, label }]
    this.onChange = onChange;
    this.formatMissing = formatMissing;
    this.value = options[0]?.value;
    this.active = 0;
    if (!root.id) root.id = nextId("combo");
    const id = root.id;
    root.classList.add("combo");
    root.innerHTML = `
      <button type="button" class="combo-button" aria-haspopup="listbox" aria-expanded="false"
        aria-labelledby="${labelledBy ?? root.dataset.label ?? ""} ${id}-value">
        <span id="${id}-value"></span>
        <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M1.5 3.5 5 7l3.5-3.5"/></svg>
      </button>
      <ul class="combo-list" role="listbox" tabindex="-1" hidden></ul>`;
    this.button = root.querySelector("button");
    this.valueEl = root.querySelector(`#${id}-value`);
    this.list = root.querySelector("ul");
    this.list.id = `${id}-list`;
    const labelId = labelledBy ?? root.dataset.label;
    if (labelId) this.list.setAttribute("aria-labelledby", labelId);
    this.button.setAttribute("aria-controls", this.list.id);
    this.button.addEventListener("click", () => (this.isOpen() ? this.close() : this.open()));
    this.button.addEventListener("keydown", (e) => {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key) && !this.isOpen()) {
        e.preventDefault();
        this.open();
      }
    });
    this.list.addEventListener("keydown", (e) => this.onKey(e));
    this.setValue(this.value);
  }

  isOpen() { return !this.list.hidden; }

  setValue(value) {
    if (!this.options.some((o) => o.value === value)) {
      if (!this.formatMissing) return;
      this.options.push({ value, label: this.formatMissing(value) });
      this.options.sort((a, b) => a.value - b.value);
    }
    this.value = value;
    this.valueEl.textContent = this.options.find((o) => o.value === value).label;
    this.valueEl.title = this.valueEl.textContent;
  }

  open() {
    if (Combo.current && Combo.current !== this) Combo.current.close(false);
    this.list.innerHTML = "";
    this.options.forEach((o, i) => {
      const li = document.createElement("li");
      li.setAttribute("role", "option");
      li.id = `${this.root.id}-opt-${i}`;
      li.textContent = o.label;
      li.title = o.label;
      li.setAttribute("aria-selected", o.value === this.value ? "true" : "false");
      li.addEventListener("pointermove", () => this.highlight(i));
      li.addEventListener("click", () => this.choose(i));
      this.list.append(li);
    });
    // Position first, invisibly, so it never shows (or lays out) at its old place for a frame.
    this.list.style.visibility = "hidden";
    Combo.layer().append(this.list);
    this.list.hidden = false;
    Combo.current = this;
    this.button.setAttribute("aria-expanded", "true");
    this.highlight(Math.max(0, this.options.findIndex((o) => o.value === this.value)));
    this.place();
    this.list.style.visibility = "";
    this.list.focus({ preventScroll: true });
  }

  /// Like Windows 11: the current item sits over the button, kept inside the window.
  place() {
    const b = this.button.getBoundingClientRect();
    const itemH = 32, pad = 5;
    const height = this.options.length * itemH + pad * 2;
    let top = b.top + b.height / 2 - (pad + this.active * itemH + itemH / 2);
    top = Math.max(8, Math.min(top, window.innerHeight - height - 8));
    this.list.style.top = `${top}px`;
    this.list.style.minWidth = `${b.width + 8}px`;
    this.list.style.maxWidth = `${window.innerWidth - 16}px`;
    this.list.style.maxHeight = `${window.innerHeight - 16}px`;
    // Long labels make the list wider than the button: measure its natural width at the left
    // edge (elsewhere it would shrink to the space left), then keep it inside the window.
    this.list.style.left = "0px";
    const width = this.list.getBoundingClientRect().width;
    this.list.style.left = `${Math.max(8, Math.min(b.left - 4, window.innerWidth - width - 8))}px`;
  }

  highlight(i) {
    this.active = i;
    [...this.list.children].forEach((li, n) => li.classList.toggle("active", n === i));
    this.list.setAttribute("aria-activedescendant", this.list.children[i]?.id ?? "");
  }

  choose(i) {
    const o = this.options[i];
    this.close();
    if (o && o.value !== this.value) {
      this.setValue(o.value);
      this.onChange(o.value);
    }
  }

  close(refocus = true) {
    if (!this.isOpen()) return;
    this.list.hidden = true;
    this.root.append(this.list); // back home, so a redrawn row never leaves a list behind
    if (Combo.current === this) Combo.current = null;
    this.button.setAttribute("aria-expanded", "false");
    if (refocus && this.button.isConnected) this.button.focus({ preventScroll: true });
  }

  onKey(e) {
    const last = this.options.length - 1;
    const keys = {
      ArrowDown: () => this.highlight(Math.min(last, this.active + 1)),
      ArrowUp: () => this.highlight(Math.max(0, this.active - 1)),
      Home: () => this.highlight(0),
      End: () => this.highlight(last),
      Enter: () => this.choose(this.active),
      " ": () => this.choose(this.active),
      Escape: () => this.close(),
      Tab: () => this.close(),
    };
    if (keys[e.key]) {
      if (e.key !== "Tab") e.preventDefault();
      e.stopPropagation();
      keys[e.key]();
    }
  }
}

document.addEventListener("pointerdown", (e) => {
  const c = Combo.current;
  if (c && !c.root.contains(e.target) && !c.list.contains(e.target)) c.close(false);
});
// Like Windows: scrolling the page (or any scrolling area) or turning the wheel outside the list closes it,
// so it never floats away from its button.
document.addEventListener("scroll", (e) => {
  const c = Combo.current;
  if (c && !c.list.contains(e.target)) c.close(false);
}, true);
document.addEventListener("wheel", (e) => {
  const c = Combo.current;
  if (c && !c.list.contains(e.target)) c.close(false);
}, { capture: true, passive: true });
window.addEventListener("blur", () => Combo.current?.close(false));
window.addEventListener("resize", () => Combo.current?.close(false));

/// Builds a combo inside a new <div>. Returns { el, combo }.
function comboField(options, value, onChange, labelledBy) {
  const el = h("div");
  const combo = new Combo(el, options, onChange, { labelledBy });
  combo.setValue(value);
  return { el, combo };
}

// ---------- segmented buttons (radio group) ----------

/// Returns a radiogroup element. options: [{ value, label }]
function segmented(options, value, onChange, ariaLabel) {
  const group = h("div", { class: "segmented", role: "radiogroup", "aria-label": ariaLabel });
  let current = value;
  const buttons = options.map((o, i) =>
    h("button", {
      type: "button", role: "radio",
      onclick: () => select(i, true),
      onkeydown: (e) => {
        const d = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
        if (d) { e.preventDefault(); select((i + d + options.length) % options.length, true, true); }
      },
    }, o.label));
  function select(i, notify, focus) {
    current = options[i].value;
    buttons.forEach((b, n) => {
      b.setAttribute("aria-checked", n === i ? "true" : "false");
      b.tabIndex = n === i ? 0 : -1;
    });
    if (focus) buttons[i].focus();
    if (notify) onChange?.(current);
  }
  group.append(...buttons);
  select(Math.max(0, options.findIndex((o) => o.value === value)), false);
  group.value = () => current;
  return group;
}

// ---------- modal dialog ----------

const FOCUSABLE = 'button, input, textarea, select, a[href], [tabindex]';

/// What Tab can reach inside `root`: visible, enabled, not tabindex=-1, not inert.
function tabbables(root) {
  return [...root.querySelectorAll(FOCUSABLE)].filter((n) =>
    !n.disabled && n.tabIndex >= 0 && n.offsetParent !== null && !n.closest("[inert]"));
}

const reduceMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/// One in-window dialog at a time.
/// While open: the page behind is inert and cannot scroll (its scrollbar gutter stays, so nothing moves);
/// on close the page, its scroll position and the focus come back exactly as they were.
const Dialog = {
  el: null,
  lastFocus: null,
  onClose: null,
  lock: null,
  guard: false, // true for forms: a click outside does not throw away what was typed
  dirty: false,

  /// guard: keep the dialog when the backdrop is clicked after the user changed something.
  open({ title, body, footer, onClose, wide, guard = false }) {
    // Reopening (e.g. form -> back to the list) keeps the original focus target and the page lock.
    const returnTo = this.el ? this.lastFocus : document.activeElement;
    const lock = this.el ? this.lock : this.lockPage();
    this.close(true, true);
    this.lastFocus = returnTo;
    this.lock = lock;
    this.onClose = onClose;
    this.guard = guard;
    this.dirty = false;
    const titleId = nextId("dlg-title");
    const box = h("div", { class: `dialog${wide ? " wide" : ""}`, role: "dialog", "aria-modal": "true", "aria-labelledby": titleId },
      h("div", { class: "dialog-head" },
        h("h3", { id: titleId, title }, title),
        h("button", { type: "button", class: "icon-button", "aria-label": "닫기", title: "닫기", onclick: () => this.close() }, icon("close"))),
      h("div", { class: "dialog-body" }, body),
      footer ? h("div", { class: "dialog-foot" }, footer) : null);
    const backdrop = h("div", { class: "backdrop", onpointerdown: (e) => { if (e.target === backdrop) this.outside(); } }, box);
    backdrop.addEventListener("keydown", (e) => this.onKey(e));
    // Anything typed, picked or toggled counts as a change worth keeping.
    const mark = () => { this.dirty = true; };
    box.addEventListener("input", mark);
    box.addEventListener("change", mark);
    // Capture phase: lists redraw on click, so the target must be checked before that happens.
    box.addEventListener("click", (e) => { if (e.target.closest(".dialog-body button, .dialog-body [role='option']")) mark(); }, true);
    document.body.append(backdrop);
    this.el = backdrop;
    this.focusFirst(box);
  },

  /// Replace title/body/footer without closing (e.g. picker -> step form).
  replace({ title, body, footer, guard }) {
    if (!this.el) return this.open({ title, body, footer, guard });
    const box = this.el.querySelector(".dialog");
    const h3 = box.querySelector("h3");
    h3.textContent = title;
    h3.title = title;
    box.querySelector(".dialog-body").replaceChildren(body);
    box.querySelector(".dialog-foot")?.remove();
    if (footer) box.append(h("div", { class: "dialog-foot" }, footer));
    if (guard != null) this.guard = guard;
    this.dirty = false;
    this.focusFirst(box);
  },

  focusFirst(box) {
    const auto = [...box.querySelectorAll("[autofocus]")].find((n) => n.offsetParent !== null);
    (auto ?? tabbables(box.querySelector(".dialog-body"))[0] ?? tabbables(box)[0])?.focus();
  },

  /// Backdrop click: close unless a form has unsaved changes (then just draw attention to it).
  outside() {
    if (this.guard && this.dirty) {
      const box = this.el.querySelector(".dialog");
      box.classList.remove("nudge");
      void box.offsetWidth;
      box.classList.add("nudge");
      return;
    }
    this.close();
  },

  lockPage() {
    const scroller = document.scrollingElement;
    const lock = { y: scroller.scrollTop, x: scroller.scrollLeft, inerted: [] };
    // overflow: hidden on the root; scrollbar-gutter: stable keeps the gutter, so nothing moves.
    document.documentElement.classList.add("modal-open");
    for (const el of document.body.children) {
      if (el.id === "popup-layer" || el.tagName === "SCRIPT" || el.inert) continue;
      el.inert = true;
      lock.inerted.push(el);
    }
    return lock;
  },

  unlockPage(lock) {
    if (!lock) return;
    for (const el of lock.inerted) el.inert = false;
    document.documentElement.classList.remove("modal-open");
    const restore = () => document.scrollingElement.scrollTo({ top: lock.y, left: lock.x, behavior: "instant" });
    restore();
    // A redraw right after closing (saving a step) may change heights; put the position back once more.
    requestAnimationFrame(restore);
  },

  /// silent: no onClose and no focus return. keepLock: another dialog content follows at once.
  close(silent, keepLock) {
    if (!this.el) return;
    Combo.current?.close(false);
    this.el.remove();
    this.el = null;
    const lock = keepLock ? null : this.lock;
    if (!keepLock) this.lock = null;
    const cb = this.onClose;
    this.onClose = null;
    if (!keepLock) this.unlockPage(lock);
    if (!silent) cb?.();
    // The opener may have been redrawn meanwhile; find its replacement by data-key.
    let back = this.lastFocus;
    if (back && !back.isConnected && back.dataset?.key) back = document.querySelector(`[data-key="${CSS.escape(back.dataset.key)}"]`);
    if (!silent && back?.isConnected) back.focus({ preventScroll: true });
  },

  onKey(e) {
    if (e.key === "Escape" && !e.defaultPrevented) {
      e.preventDefault();
      this.close();
      return;
    }
    if (e.key !== "Tab") return;
    const items = tabbables(this.el);
    if (!items.length) return;
    const first = items[0], last = items[items.length - 1];
    if (!this.el.contains(document.activeElement)) { e.preventDefault(); first.focus(); }
    else if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  },
};

// ---------- toast with undo ----------

/// A short message at the bottom of the window, e.g. "동작을 제거했습니다 [실행 취소]".
/// Stays ~6 s (paused while hovered or focused); Ctrl+Z also undoes while it is shown.
const Toast = {
  host: (() => {
    const el = h("div", { id: "toast-host", role: "status", "aria-live": "polite" });
    document.body.append(el);
    return el;
  })(),
  el: null,
  undoFn: null,
  timer: null,
  left: 0,
  started: 0,

  /// focusAfter(): where focus goes if the toast closes while it had focus.
  show(message, { undo, timeout = 6000, focusAfter } = {}) {
    this.dismiss();
    this.undoFn = undo ?? null;
    this.focusAfter = focusAfter ?? null;
    this.el = h("div", { class: "toast", onpointerenter: () => this.pause(), onpointerleave: () => this.resume(),
      onfocusin: () => this.pause(), onfocusout: () => this.resume() },
      h("span", { class: "toast-text" }, message),
      undo ? h("button", { type: "button", class: "button subtle", onclick: () => this.undo() }, "실행 취소") : null,
      h("button", { type: "button", class: "icon-button small", "aria-label": "알림 닫기", title: "닫기", onclick: () => this.dismiss() }, icon("close")));
    this.host.append(this.el);
    this.left = timeout;
    this.resume();
  },

  pause() {
    if (!this.timer) return;
    clearTimeout(this.timer);
    this.timer = null;
    this.left -= performance.now() - this.started;
  },

  resume() {
    if (!this.el || this.timer) return;
    this.started = performance.now();
    this.timer = setTimeout(() => this.dismiss(), Math.max(1500, this.left));
  },

  dismiss() {
    clearTimeout(this.timer);
    this.timer = null;
    const hadFocus = this.el?.contains(document.activeElement);
    this.el?.remove();
    this.el = null;
    this.undoFn = null;
    const after = this.focusAfter;
    this.focusAfter = null;
    // homeFocus() (app.js): the first control of the view on screen.
    if (hadFocus) (after?.() ?? (typeof homeFocus === "function" ? homeFocus() : null))?.focus();
  },

  /// Returns true when there was something to undo.
  undo() {
    const fn = this.undoFn;
    if (!fn) return false;
    this.dismiss();
    fn();
    return true;
  },
};

// Ctrl+Z undoes the last removal while its message is shown (not while typing in a field).
window.addEventListener("keydown", (e) => {
  if (!(e.ctrlKey && !e.altKey && !e.shiftKey && e.code === "KeyZ") || Dialog.el) return;
  if (e.target.closest?.("input, textarea, [contenteditable]")) return;
  if (Toast.undo()) e.preventDefault();
});

// ---------- shortcut text ----------

const MODIFIER_CODES = new Set([
  "ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight",
]);

/// Tauri shortcut spec from a keydown event, or null for a lone modifier.
function specFromEvent(e) {
  if (MODIFIER_CODES.has(e.code)) return null;
  const mods = [];
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  return { mods, spec: [...mods, e.code].join("+") };
}

const KEY_NAMES = {
  Control: "Ctrl", Super: "Win", Meta: "Win", Backquote: "`", Minus: "-", Equal: "=", BracketLeft: "[",
  BracketRight: "]", Backslash: "\\", Semicolon: ";", Quote: "'", Comma: ",", Period: ".", Slash: "/",
  ArrowRight: "→", ArrowLeft: "←", ArrowUp: "↑", ArrowDown: "↓", Escape: "Esc", Enter: "Enter",
  Space: "Space", Backspace: "Backspace", Delete: "Delete", Tab: "Tab", PrintScreen: "PrtSc",
};

/// "Super+Control+ArrowRight" -> "Win + Ctrl + →"
function specLabel(spec) {
  return spec.split("+").map((p) => {
    if (KEY_NAMES[p]) return KEY_NAMES[p];
    if (/^Key[A-Z]$/.test(p)) return p.slice(3);
    if (/^Digit\d$/.test(p)) return p.slice(5);
    if (/^Numpad\d$/.test(p)) return "Num " + p.slice(6);
    return p;
  }).join(" + ");
}

// ---------- shortcuts that many apps already use ----------

const MOD_ORDER = ["Control", "Alt", "Shift", "Super"];

/// "Shift+Control+KeyZ" -> "Control+Shift+KeyZ" (same order everywhere, so specs compare equal).
function normalizeSpec(spec) {
  const parts = spec.split("+").map((p) => ({ Ctrl: "Control", Meta: "Super", Command: "Super" }[p] ?? p));
  const mods = MOD_ORDER.filter((m) => parts.includes(m));
  const key = parts.filter((p) => !MOD_ORDER.includes(p)).pop() ?? "";
  return [...mods, key].join("+");
}

const COMMON_SHORTCUTS = (() => {
  const set = new Set();
  const add = (mods, keys) => keys.forEach((k) => set.add(normalizeSpec([...mods, k].join("+"))));
  const letters = "A C V X Z Y S F P N T W R O Q H L D E B I U K J".split(" ").map((c) => `Key${c}`);
  const digits = Array.from({ length: 10 }, (_, i) => `Digit${i}`);
  add(["Control"], [...letters, "Tab", "Enter", "Space", "Equal", "Minus", ...digits, "F4", "F5"]);
  add(["Control", "Shift"], ["KeyT", "KeyN", "KeyZ", "KeyS", "Tab", "Escape"]);
  add(["Alt"], ["F4", "Tab", "Space", "Enter"]);
  add(["Shift"], ["Delete", "F10"]);
  add([], Array.from({ length: 12 }, (_, i) => `F${i + 1}`));
  return set;
})();

/// True for combinations most apps (or Windows) already use, e.g. Ctrl+C, Alt+F4, Win+E.
function isCommonShortcut(spec) {
  const n = normalizeSpec(spec);
  return COMMON_SHORTCUTS.has(n) || /^Super\+(Key[A-Z]|Digit\d)$/.test(n);
}

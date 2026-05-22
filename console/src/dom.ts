// Tiny DOM helpers. The console is small enough to live without React or
// Svelte. h(...) builds an HTMLElement, mount() swaps it into a container.

type Child = Node | string | number | null | undefined | false;
type Attrs = Record<string, unknown> & {
  events?: Record<string, EventListener>;
  dataset?: Record<string, string>;
};

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  children: Child | Child[] = [],
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (k === "events" && typeof v === "object") {
      for (const [ev, handler] of Object.entries(v as Record<string, EventListener>)) {
        el.addEventListener(ev, handler);
      }
      continue;
    }
    if (k === "dataset" && typeof v === "object") {
      for (const [dk, dv] of Object.entries(v as Record<string, string>)) {
        el.dataset[dk] = dv;
      }
      continue;
    }
    if (k === "class") {
      el.className = String(v);
      continue;
    }
    if (k === "style" && typeof v === "object") {
      Object.assign(el.style, v as Record<string, string>);
      continue;
    }
    if (v === true) {
      el.setAttribute(k, "");
      continue;
    }
    if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
      continue;
    }
    el.setAttribute(k, String(v));
  }
  const list = Array.isArray(children) ? children : [children];
  for (const child of list) {
    if (child === null || child === undefined || child === false) continue;
    if (typeof child === "string" || typeof child === "number") {
      el.appendChild(document.createTextNode(String(child)));
    } else {
      el.appendChild(child);
    }
  }
  return el;
}

export function mount(container: HTMLElement, node: Node): void {
  container.replaceChildren(node);
}

export function clear(container: HTMLElement): void {
  container.replaceChildren();
}

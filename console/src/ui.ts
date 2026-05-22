import { h } from "./dom";

export function th(label: string): HTMLElement {
  return h("th", {}, label);
}

export function selectInput(options: string[], placeholder?: string): HTMLSelectElement {
  const sel = h("select") as HTMLSelectElement;
  for (const o of options) {
    const opt = h("option", { value: o }, o || placeholder || "");
    sel.appendChild(opt);
  }
  return sel;
}

export function formGroup(label: string, control: HTMLElement): HTMLElement {
  return h("div", { class: "form-group" }, [
    h("label", {}, label),
    control,
  ]);
}

export function kvRow(k: string, v: string | HTMLElement): HTMLElement {
  return h("div", { style: { display: "contents" } }, [
    h("div", { class: "k" }, k),
    h("div", { class: "v" }, typeof v === "string" ? v : "") as HTMLElement,
  ]);
}

export function kv(rows: Array<[string, string | HTMLElement]>): HTMLElement {
  const wrap = h("div", { class: "kv" });
  for (const [k, v] of rows) {
    wrap.appendChild(h("div", { class: "k" }, k));
    const cell = h("div", { class: "v" });
    if (typeof v === "string") {
      cell.textContent = v;
    } else {
      cell.appendChild(v);
    }
    wrap.appendChild(cell);
  }
  return wrap;
}

export interface ActionDef {
  label: string;
  kind?: "primary" | "danger" | "ghost";
  onClick: () => void | Promise<void>;
}

export function openDrawer(
  title: string,
  body: HTMLElement,
  actions: ActionDef[],
): () => void {
  const drawer = h("aside", { class: "drawer" });
  const backdrop = h("div", {
    class: "drawer-backdrop",
    events: {
      click: (e) => {
        if ((e.target as HTMLElement) === backdrop) onClose();
      },
    },
  }, drawer);
  const onClose = (): void => backdrop.remove();
  drawer.replaceChildren(
    h("div", { style: { display: "flex", justifyContent: "space-between", alignItems: "center" } }, [
      h("h2", {}, title),
      h("button", { class: "ghost", events: { click: () => onClose() } }, "X"),
    ]),
    body,
    h("div", { class: "actions" }, actions.map((a) =>
      h("button", { class: a.kind ?? "", events: { click: () => void a.onClick() } }, a.label),
    )),
  );
  document.body.appendChild(backdrop);
  return onClose;
}

export function openModal(
  title: string,
  bodyEls: HTMLElement[],
  actions: ActionDef[],
): () => void {
  const modal = h("div", { class: "modal" });
  const backdrop = h("div", {
    class: "modal-backdrop",
    events: {
      click: (e) => {
        if ((e.target as HTMLElement) === backdrop) onClose();
      },
    },
  }, modal);
  const onClose = (): void => backdrop.remove();
  modal.replaceChildren(
    h("h2", { style: { margin: 0, fontSize: "16px" } }, title),
    ...bodyEls,
    h(
      "div",
      { class: "actions", style: { display: "flex", gap: "8px", justifyContent: "flex-end" } },
      actions.map((a) =>
        h("button", { class: a.kind ?? "", events: { click: () => void a.onClick() } }, a.label),
      ),
    ),
  );
  document.body.appendChild(backdrop);
  return onClose;
}

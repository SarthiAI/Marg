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

/**
 * Pop a modal that explains the current Kavach mode in plain language and
 * tells the operator how to switch. Used wherever the mode badge appears
 * (topbar, policy page, audit page) so a single click on the badge gives a
 * complete answer.
 */
export function openKavachModeInfo(mode: string): () => void {
  const m = (mode || "?").toLowerCase();
  const headerLabel =
    m === "enforce"
      ? "Kavach is live (enforce mode)"
      : m === "observe"
        ? "Kavach is in observe mode (logging only)"
        : `Kavach mode: ${mode}`;

  const body: HTMLElement[] = [];

  if (m === "observe") {
    body.push(
      h("p", { style: { margin: "8px 0", lineHeight: "1.5" } }, [
        "Every request still goes through the Kavach policy gate, and every verdict ",
        "is recorded in the signed audit chain, but ",
        h("strong", {}, "nothing is blocked"),
        ". Use this mode to see what your policy would refuse against real traffic ",
        "before you turn enforcement on. ",
        "Open the ",
        h("strong", {}, "Audit"),
        " page to see the ",
        h("em", {}, "would-refuse"),
        " events that observe mode produces.",
      ]),
      h("h3", { style: { fontSize: "13px", marginTop: "14px", marginBottom: "6px" } }, "Going live (switch to enforce)"),
      h("ol", { style: { paddingLeft: "20px", lineHeight: "1.6" } }, [
        h("li", {}, [
          "Edit ",
          h("code", {}, "marg.toml"),
          ", change ",
          h("code", {}, '[kavach].mode = "observe"'),
          " to ",
          h("code", {}, '[kavach].mode = "enforce"'),
          ".",
        ]),
        h("li", {}, [
          "Reload without restart: ",
          h("code", {}, "kill -HUP <marg-pid>"),
          " or call ",
          h("code", {}, "POST /admin/policy/reload"),
          ".",
        ]),
        h("li", {}, [
          "Verify in the topbar. The badge flips to ",
          h("strong", {}, "enforce"),
          " on success.",
        ]),
      ]),
      h("p", { class: "help-block", style: { marginTop: "10px" } },
        "Tip: clear the Audit would-refuse list first. Anything in there will start returning 403 the moment you flip the mode."),
    );
  } else if (m === "enforce") {
    body.push(
      h("p", { style: { margin: "8px 0", lineHeight: "1.5" } }, [
        "Default-deny is active. Any request that does not match a permit rule ",
        "in your policy is refused with HTTP 403 and the header ",
        h("code", {}, "x-marg-reason: kavach_refused"),
        ". Every verdict is appended to the signed audit chain so the refusal ",
        "is provable to a third party.",
      ]),
      h("h3", { style: { fontSize: "13px", marginTop: "14px", marginBottom: "6px" } }, "Switching back to observe"),
      h("ol", { style: { paddingLeft: "20px", lineHeight: "1.6" } }, [
        h("li", {}, [
          "Edit ",
          h("code", {}, "marg.toml"),
          ", change ",
          h("code", {}, '[kavach].mode = "enforce"'),
          " to ",
          h("code", {}, '[kavach].mode = "observe"'),
          ".",
        ]),
        h("li", {}, [
          "Reload: ",
          h("code", {}, "kill -HUP <marg-pid>"),
          " or ",
          h("code", {}, "POST /admin/policy/reload"),
          ".",
        ]),
      ]),
    );
  } else {
    body.push(
      h("p", {}, "Kavach mode could not be read. Check that Marg is running and that the admin token is still valid."),
    );
  }

  let close: () => void = () => {};
  close = openModal(headerLabel, body, [
    { label: "Got it", kind: "primary", onClick: () => close() },
  ]);
  return close;
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

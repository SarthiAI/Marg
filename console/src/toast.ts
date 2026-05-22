import { h } from "./dom";

type Kind = "ok" | "warn" | "error";

let container: HTMLElement | null = null;

function ensureContainer(): HTMLElement {
  if (container && document.body.contains(container)) return container;
  container = h("div", { id: "toasts" });
  document.body.appendChild(container);
  return container;
}

export function toast(message: string, kind: Kind = "ok", sticky = false): void {
  const root = ensureContainer();
  const item = h("div", { class: `toast toast-${kind}` }, [
    h("div", { class: "toast-message" }, message),
    h("button", {
      class: "toast-close",
      "aria-label": "Dismiss",
      events: {
        click: () => item.remove(),
      },
    }, "X"),
  ]);
  root.appendChild(item);
  if (!sticky) {
    setTimeout(() => item.remove(), 5000);
  }
}

export function toastError(prefix: string, e: unknown): void {
  const msg = e instanceof Error ? e.message : String(e);
  const code = e && typeof e === "object" && "code" in e ? `[${(e as { code: string }).code}] ` : "";
  toast(`${prefix}: ${code}${msg}`, "error", true);
}

import { createAdminToken, listAdminTokens, revokeAdminToken } from "../api";
import { h, mount } from "../dom";
import { fmtIso } from "../format";
import { toast, toastError } from "../toast";
import { adminTokenStatus, type AdminTokenRecord } from "../types";
import { formGroup, openDrawer, openModal, th } from "../ui";

export async function renderAdminTokens(target: HTMLElement, signal: AbortSignal): Promise<void> {
  const tbody = h("tbody");
  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Admin tokens"),
      h("div", { class: "controls" }, [
        h("button", { class: "ghost", events: { click: () => refresh() } }, "Refresh"),
        h("button", { class: "primary", events: { click: () => openCreate(refresh) } }, "Create token"),
      ]),
    ]),
    h("p", { class: "help-block" }, [
      "Rotation flow: create a new token, copy it into your operator process, sign in here with the new token, then revoke the old one. ",
      "Marg's auth cache TTL is 5 seconds, so revokes propagate within that window.",
    ]),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Prefix"),
          th("Label"),
          th("Status"),
          th("Created"),
          th("Revoked"),
          th(""),
        ])),
        tbody,
      ]),
    ]),
  ]);
  mount(target, layout);

  async function refresh(): Promise<void> {
    try {
      const { tokens } = await listAdminTokens(signal);
      tbody.replaceChildren();
      if (tokens.length === 0) {
        tbody.appendChild(h("tr", {}, h("td", { colspan: 6, class: "empty" }, "No admin tokens. Something is wrong, there should always be at least one.")));
        return;
      }
      for (const t of tokens) tbody.appendChild(row(t));
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Failed to load tokens", e);
    }
  }

  function row(t: AdminTokenRecord): HTMLElement {
    const status = adminTokenStatus(t);
    const actions = h("div", { class: "row-actions" }, [
      status === "active"
        ? h("button", {
            class: "danger",
            events: { click: () => confirmRevoke(t, refresh) },
          }, "Revoke")
        : h("span", { class: "mono", style: { color: "var(--muted)", fontSize: "11px" } }, "revoked"),
    ]);
    return h("tr", {}, [
      h("td", {}, h("code", {}, t.token_prefix)),
      h("td", {}, t.label || "(no label)"),
      h("td", {}, h("span", { class: `badge ${status}` }, status)),
      h("td", { class: "mono" }, fmtIso(t.created_at)),
      h("td", { class: "mono" }, t.revoked_at ? fmtIso(t.revoked_at) : "-"),
      h("td", {}, actions),
    ]);
  }

  await refresh();
}

function openCreate(onCreated: () => void | Promise<void>): void {
  const label = h("input", { type: "text", placeholder: "e.g. ci-bot, ops-laptop" }) as HTMLInputElement;
  const body = h("div", {}, [
    formGroup("Label (optional)", label),
    h("p", { class: "help-block" }, "The plaintext token will be shown once. Save it before closing the next dialog."),
  ]);
  let onClose = () => undefined as void;
  onClose = openDrawer("Create admin token", body, [
    { label: "Cancel", kind: "ghost", onClick: () => onClose() },
    {
      label: "Create",
      kind: "primary",
      onClick: async () => {
        try {
          const r = await createAdminToken(label.value.trim() || undefined);
          onClose();
          await onCreated();
          showTokenModal(r.token);
        } catch (e) {
          toastError("Create failed", e);
        }
      },
    },
  ]);
}

function showTokenModal(token: string): void {
  let onClose = () => undefined as void;
  onClose = openModal("Token created", [
    h("p", {}, "Save this token now. It is shown only once."),
    h("div", { class: "token-display" }, token),
  ], [
    {
      label: "Copy",
      kind: "ghost",
      onClick: async () => {
        try {
          await navigator.clipboard.writeText(token);
          toast("Token copied to clipboard", "ok");
        } catch (e) {
          toastError("Copy failed", e);
        }
      },
    },
    { label: "Done", kind: "primary", onClick: () => onClose() },
  ]);
}

function confirmRevoke(t: AdminTokenRecord, onDone: () => void | Promise<void>): void {
  const confirmInput = h("input", { type: "text", placeholder: t.token_prefix }) as HTMLInputElement;
  let onClose = () => undefined as void;
  onClose = openModal("Revoke admin token", [
    h("p", {}, [
      "Type the prefix ",
      h("code", {}, t.token_prefix),
      " below to confirm. The revocation will take effect within 5 seconds (auth cache TTL).",
    ]),
    confirmInput,
  ], [
    { label: "Cancel", kind: "ghost", onClick: () => onClose() },
    {
      label: "Revoke",
      kind: "danger",
      onClick: async () => {
        if (confirmInput.value.trim() !== t.token_prefix) {
          toast("Confirmation text did not match", "error");
          return;
        }
        try {
          await revokeAdminToken(t.id);
          toast(`Token ${t.token_prefix} revoked`, "ok");
          onClose();
          await onDone();
        } catch (e) {
          toastError("Revoke failed", e);
        }
      },
    },
  ]);
}

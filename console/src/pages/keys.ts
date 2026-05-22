import {
  createKey,
  getBudget,
  getKey,
  invalidateKey,
  listKeys,
  listRequests,
  revokeKey,
} from "../api";
import { h, mount } from "../dom";
import { fmtIso, fmtMs, fmtUsd, shortId } from "../format";
import { navigate } from "../router";
import { toast, toastError } from "../toast";
import type { MargKey } from "../types";
import { formGroup, kv, openDrawer, openModal, selectInput, th } from "../ui";

export async function renderKeysList(target: HTMLElement, signal: AbortSignal): Promise<void> {
  const principalInput = h("input", { type: "text", placeholder: "principal id" }) as HTMLInputElement;
  const kindSelect = selectInput(["", "user", "service", "agent"], "any kind");
  const statusSelect = selectInput(["", "active", "revoked"], "any status");

  const tbody = h("tbody");
  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Keys"),
      h("div", { class: "controls" }, [
        h("button", {
          class: "primary",
          events: { click: () => openCreateDrawer(() => refresh()) },
        }, "Create key"),
      ]),
    ]),
    h("div", { class: "filter-row" }, [
      formGroup("Principal", principalInput),
      formGroup("Kind", kindSelect),
      formGroup("Status", statusSelect),
      h("button", { events: { click: () => refresh() } }, "Apply filters"),
      h("button", { class: "ghost", events: { click: () => refresh() } }, "Refresh"),
    ]),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Prefix"),
          th("Principal"),
          th("Kind"),
          th("Team"),
          th("Status"),
          th("Created"),
        ])),
        tbody,
      ]),
    ]),
  ]);

  mount(target, layout);

  async function refresh(): Promise<void> {
    try {
      const res = await listKeys({
        principal: principalInput.value.trim() || undefined,
        kind: kindSelect.value || undefined,
        status: statusSelect.value || undefined,
      }, signal);
      tbody.replaceChildren();
      if (res.keys.length === 0) {
        tbody.appendChild(h("tr", {}, h("td", { colspan: 6, class: "empty" }, "No keys match the filters.")));
        return;
      }
      for (const k of res.keys) {
        const tr = h("tr", {
          class: "clickable",
          events: { click: () => navigate(`/keys/${encodeURIComponent(k.id)}`) },
        }, [
          h("td", {}, h("code", {}, k.token_prefix ?? shortId(k.id, 10))),
          h("td", {}, k.principal.id),
          h("td", {}, k.principal.kind),
          h("td", {}, k.team ?? "-"),
          h("td", {}, h("span", { class: `badge ${k.status}` }, k.status)),
          h("td", { class: "mono" }, fmtIso(k.created_at)),
        ]);
        tbody.appendChild(tr);
      }
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Failed to load keys", e);
    }
  }

  await refresh();
}

export async function renderKeyDetail(
  target: HTMLElement,
  id: string,
  signal: AbortSignal,
): Promise<void> {
  if (!id) {
    target.textContent = "Missing key id";
    return;
  }
  try {
    const [{ key }, budget, reqs] = await Promise.all([
      getKey(id, signal),
      getBudget(id, signal),
      listRequests({ key_id: id, limit: 50 }, signal),
    ]);

    const reqsBody = h("tbody");
    for (const r of reqs.entries) {
      reqsBody.appendChild(h("tr", {}, [
        h("td", { class: "mono" }, fmtIso(r.timestamp)),
        h("td", {}, r.model),
        h("td", {}, r.provider),
        h("td", {}, h("span", { class: `badge ${r.status >= 400 ? "err" : "ok"}` }, String(r.status))),
        h("td", {}, fmtMs(r.latency_ms)),
        h("td", {}, fmtUsd(r.cost_usd)),
      ]));
    }
    if (reqs.entries.length === 0) {
      reqsBody.appendChild(h("tr", {}, h("td", { colspan: 6, class: "empty" }, "No requests recorded for this key.")));
    }

    const layout = h("div", {}, [
      h("div", { class: "page-header" }, [
        h("h1", {}, [
          "Key ",
          h("code", {}, key.token_prefix ?? shortId(key.id, 10)),
        ]),
        h("div", { class: "controls" }, [
          h("button", { class: "ghost", events: { click: () => navigate("/keys") } }, "Back"),
          h("button", {
            events: {
              click: async () => {
                try {
                  await invalidateKey(key.id, signal);
                  toast("Hot store entry invalidated", "ok");
                } catch (e) {
                  toastError("Invalidate failed", e);
                }
              },
            },
          }, "Invalidate cache"),
          h("button", {
            class: "danger",
            events: {
              click: () => confirmRevokeKey(key, () => navigate("/keys")),
            },
          }, "Revoke key"),
        ]),
      ]),
      keyOverviewCard(key),
      budgetCard(budget),
      h("div", { class: "card", style: { marginTop: "16px" } }, [
        h("h3", { style: { marginTop: 0, fontSize: "14px" } }, "Recent requests (50)"),
        h("div", { class: "table-wrap" }, [
          h("table", {}, [
            h("thead", {}, h("tr", {}, [
              th("When"),
              th("Model"),
              th("Provider"),
              th("Status"),
              th("Latency"),
              th("Cost"),
            ])),
            reqsBody,
          ]),
        ]),
      ]),
    ]);

    mount(target, layout);
  } catch (e) {
    if ((e as { name?: string }).name === "AbortError") return;
    toastError("Failed to load key", e);
    target.textContent = `Failed to load key ${id}`;
  }
}

function openCreateDrawer(onCreated: () => void): void {
  const principal = h("input", { type: "text", required: true, placeholder: "alice@example.com" }) as HTMLInputElement;
  const kind = selectInput(["user", "service", "agent"]);
  const team = h("input", { type: "text", placeholder: "optional team" }) as HTMLInputElement;
  const budget = h("input", { type: "number", min: "0", step: "0.01", value: "0" }) as HTMLInputElement;
  const rpm = h("input", { type: "number", min: "0", step: "1", value: "0" }) as HTMLInputElement;

  const body = h("div", {}, [
    formGroup("Principal id", principal),
    formGroup("Kind", kind),
    formGroup("Team (optional)", team),
    formGroup("Daily budget USD (0 = unlimited)", budget),
    formGroup("RPM (0 = unlimited)", rpm),
  ]);

  let onClose = () => undefined as void;
  onClose = openDrawer("Create key", body, [
    { label: "Cancel", kind: "ghost", onClick: () => onClose() },
    {
      label: "Create",
      kind: "primary",
      onClick: async () => {
        if (!principal.value.trim()) {
          toast("Principal id is required", "error");
          return;
        }
        try {
          const r = await createKey({
            principal_id: principal.value.trim(),
            kind: kind.value,
            team: team.value.trim() || null,
            daily_budget_usd: Number(budget.value) || 0,
            rpm: Number(rpm.value) || 0,
          });
          onClose();
          onCreated();
          showOneTimeTokenModal(r.token, r.key);
        } catch (e) {
          toastError("Create failed", e);
        }
      },
    },
  ]);
}

function confirmRevokeKey(key: MargKey, onDone: () => void): void {
  const want = key.token_prefix ?? shortId(key.id, 10);
  const confirmInput = h("input", { type: "text", placeholder: want }) as HTMLInputElement;
  let onClose = () => undefined as void;
  onClose = openModal("Revoke key", [
    h("p", {}, [
      "This is irreversible. To confirm, type the prefix ",
      h("code", {}, want),
      " below.",
    ]),
    confirmInput,
  ], [
    { label: "Cancel", kind: "ghost", onClick: () => onClose() },
    {
      label: "Revoke",
      kind: "danger",
      onClick: async () => {
        if (confirmInput.value.trim() !== want) {
          toast("Confirmation text did not match", "error");
          return;
        }
        try {
          await revokeKey(key.id);
          toast(`Key ${want} revoked`, "ok");
          onClose();
          onDone();
        } catch (e) {
          toastError("Revoke failed", e);
        }
      },
    },
  ]);
}

function showOneTimeTokenModal(token: string, key: MargKey): void {
  let onClose = () => undefined as void;
  onClose = openModal("Key created", [
    h("p", {}, "Save this token now. It is shown only once."),
    h("div", { class: "token-display" }, token),
    h("p", { style: { color: "var(--muted)", fontSize: "12px" } }, [
      "Principal: ",
      h("code", {}, key.principal.id),
      " (",
      key.principal.kind,
      ")",
    ]),
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

function keyOverviewCard(key: MargKey): HTMLElement {
  return h("div", { class: "card" }, [
    h("h3", { style: { marginTop: 0, fontSize: "14px" } }, "Overview"),
    kv([
      ["Id", key.id],
      ["Prefix", key.token_prefix ?? "-"],
      ["Principal", `${key.principal.id} (${key.principal.kind})`],
      ["Team", key.team ?? "-"],
      ["Status", key.status],
      ["Created at", fmtIso(key.created_at)],
      ["Revoked at", key.revoked_at ? fmtIso(key.revoked_at) : "-"],
    ]),
  ]);
}

function budgetCard(snap: {
  budget: { daily_usd: number; rpm: number } | null;
  day: string;
  spent_usd: number;
  remaining_usd: number | null;
}): HTMLElement {
  return h("div", { class: "card", style: { marginTop: "16px" } }, [
    h("h3", { style: { marginTop: 0, fontSize: "14px" } }, `Budget for ${snap.day}`),
    kv([
      ["Daily USD cap", snap.budget ? fmtUsd(snap.budget.daily_usd) : "(no budget)"],
      ["Today's spend", fmtUsd(snap.spent_usd)],
      ["Remaining", fmtUsd(snap.remaining_usd)],
      ["RPM cap", snap.budget ? String(snap.budget.rpm) : "-"],
    ]),
  ]);
}

import { getBudget, listKeys, upsertBudget } from "../api";
import { h, mount } from "../dom";
import { fmtUsd, shortId } from "../format";
import { navigate } from "../router";
import { toast, toastError } from "../toast";
import { formGroup, openDrawer, th } from "../ui";

export async function renderBudgets(target: HTMLElement, signal: AbortSignal): Promise<void> {
  const tbody = h("tbody");
  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Budgets"),
      h("div", { class: "controls" }, [
        h("button", { class: "ghost", events: { click: () => refresh() } }, "Refresh"),
        h("button", { class: "primary", events: { click: () => openUpsertDrawer(refresh) } }, "Set budget"),
      ]),
    ]),
    h("p", { class: "help-block" }, "Click a cap or RPM value to edit it inline."),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Key"),
          th("Principal"),
          th("Team"),
          th("Daily USD"),
          th("Spent today"),
          th("Remaining today"),
          th("RPM"),
        ])),
        tbody,
      ]),
    ]),
  ]);
  mount(target, layout);

  async function refresh(): Promise<void> {
    try {
      const { keys } = await listKeys({ status: "active" }, signal);
      tbody.replaceChildren();
      if (keys.length === 0) {
        tbody.appendChild(h("tr", {}, h("td", { colspan: 7, class: "empty" }, "No active keys.")));
        return;
      }
      for (const k of keys) {
        try {
          const snap = await getBudget(k.id, signal);
          tbody.appendChild(renderRow(k, snap));
        } catch (e) {
          if ((e as { name?: string }).name === "AbortError") return;
        }
      }
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Failed to load budgets", e);
    }
  }

  function renderRow(
    k: { id: string; token_prefix?: string; principal: { id: string }; team?: string | null },
    snap: {
      budget: { daily_usd: number; rpm: number } | null;
      spent_usd: number;
      remaining_usd: number | null;
    },
  ): HTMLElement {
    const capCell = inlineEditCell(
      snap.budget ? fmtUsd(snap.budget.daily_usd) : "(none)",
      async (val) => {
        const n = Number(val);
        if (isNaN(n) || n < 0) {
          toast("Daily USD must be a non-negative number", "error");
          return false;
        }
        try {
          await upsertBudget({ key_id: k.id, daily_usd: n, rpm: snap.budget?.rpm ?? 0 });
          toast("Budget updated", "ok");
          await refresh();
          return true;
        } catch (e) {
          toastError("Update failed", e);
          return false;
        }
      },
    );
    const rpmCell = inlineEditCell(
      String(snap.budget?.rpm ?? 0),
      async (val) => {
        const n = Number(val);
        if (isNaN(n) || n < 0 || !Number.isInteger(n)) {
          toast("RPM must be a non-negative integer", "error");
          return false;
        }
        try {
          await upsertBudget({ key_id: k.id, daily_usd: snap.budget?.daily_usd ?? 0, rpm: n });
          toast("Budget updated", "ok");
          await refresh();
          return true;
        } catch (e) {
          toastError("Update failed", e);
          return false;
        }
      },
    );
    return h("tr", {}, [
      h("td", {}, h("a", { href: `#/keys/${encodeURIComponent(k.id)}` }, k.token_prefix ?? shortId(k.id, 10))),
      h("td", {}, k.principal.id),
      h("td", {}, k.team ?? "-"),
      capCell,
      h("td", {}, fmtUsd(snap.spent_usd)),
      h("td", {}, fmtUsd(snap.remaining_usd)),
      rpmCell,
    ]);
  }

  function inlineEditCell(initialText: string, onCommit: (newValue: string) => Promise<boolean>): HTMLElement {
    const td = h("td", { style: { cursor: "pointer" } }) as HTMLTableCellElement;
    const view = h("span", {}, initialText);
    td.appendChild(view);
    td.addEventListener("click", () => startEdit());
    function startEdit(): void {
      const input = h("input", { type: "text", value: initialText }) as HTMLInputElement;
      input.style.width = "100%";
      td.replaceChildren(input);
      input.focus();
      input.select();
      const commit = async (): Promise<void> => {
        const val = input.value.trim();
        if (val === initialText) {
          td.replaceChildren(view);
          return;
        }
        const ok = await onCommit(val);
        if (!ok) td.replaceChildren(view);
      };
      input.addEventListener("blur", () => void commit());
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          void commit();
        } else if (e.key === "Escape") {
          td.replaceChildren(view);
        }
      });
    }
    return td;
  }

  function openUpsertDrawer(onDone: () => void | Promise<void>): void {
    const keyId = h("input", { type: "text", placeholder: "key id (uuid)" }) as HTMLInputElement;
    const daily = h("input", { type: "number", min: "0", step: "0.01", value: "0" }) as HTMLInputElement;
    const rpm = h("input", { type: "number", min: "0", step: "1", value: "0" }) as HTMLInputElement;
    const lookupBtn = h(
      "button",
      {
        class: "ghost",
        events: {
          click: () => {
            const id = keyId.value.trim();
            if (!id) return;
            navigate(`/keys/${encodeURIComponent(id)}`);
          },
        },
      },
      "Open in Keys",
    );
    const body = h("div", {}, [
      formGroup("Key id", keyId),
      formGroup("Daily USD cap", daily),
      formGroup("RPM cap", rpm),
      h("div", { class: "help-block" }, [
        "Find the key id on the Keys page. ",
        lookupBtn,
      ]),
    ]);
    let onClose = () => undefined as void;
    onClose = openDrawer("Set budget", body, [
      { label: "Cancel", kind: "ghost", onClick: () => onClose() },
      {
        label: "Save",
        kind: "primary",
        onClick: async () => {
          if (!keyId.value.trim()) {
            toast("Key id is required", "error");
            return;
          }
          try {
            await upsertBudget({
              key_id: keyId.value.trim(),
              daily_usd: Number(daily.value) || 0,
              rpm: Number(rpm.value) || 0,
            });
            toast("Budget saved", "ok");
            onClose();
            await onDone();
          } catch (e) {
            toastError("Save failed", e);
          }
        },
      },
    ]);
  }

  await refresh();
}

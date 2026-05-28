import { getKavachStatus, listKavachAuditEntries, verifyAuditChain } from "../api";
import { h, mount } from "../dom";
import { fmtIso, shortId } from "../format";
import { toast, toastError } from "../toast";
import type { KavachAuditEntryView } from "../types";
import { kv, openKavachModeInfo, th } from "../ui";

const PAGE_LIMIT = 100;

export async function renderAudit(target: HTMLElement, signal: AbortSignal): Promise<void> {
  const statusInfo = h("div", { class: "card" });
  const entriesBody = h("tbody");

  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Audit chain"),
      h("div", { class: "controls" }, [
        h("button", { class: "ghost", events: { click: () => refresh() } }, "Refresh"),
        h("button", {
          class: "primary",
          events: {
            click: async () => {
              try {
                const result = await verifyAuditChain({}, signal);
                if (result.verified) {
                  toast(`Chain verified: ${result.count} entries`, "ok");
                } else {
                  toastError("Chain verify failed", new Error(result.error ?? "unknown"));
                }
              } catch (e) {
                toastError("Verify failed", e);
              }
            },
          },
        }, "Verify"),
      ]),
    ]),
    statusInfo,
    h("h3", { style: { marginTop: "16px", fontSize: "14px" } }, `Last ${PAGE_LIMIT} entries`),
    h("p", { class: "help-block" }, "Each row is one signed entry. The data column shows the marg.* event packed inside the signature envelope."),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Index"),
          th("Signed at"),
          th("Schema"),
          th("Verdict"),
          th("Principal"),
          th("Action"),
          th("Reason"),
        ])),
        entriesBody,
      ]),
    ]),
  ]);

  mount(target, layout);

  async function refresh(): Promise<void> {
    try {
      const status = await getKavachStatus(signal);
      const isEnforce = status.mode === "enforce";
      statusInfo.replaceChildren(
        h("h3", { style: { marginTop: 0, fontSize: "14px" } }, [
          "Kavach status ",
          h("span", {
            class: `badge ${isEnforce ? "code" : "ok"}`,
            title: status.mode === "observe"
              ? "Observe mode: every verdict is logged but nothing is blocked. Click for the recipe to go live."
              : status.mode === "enforce"
                ? "Enforce mode: default-deny is active. Click for details."
                : "Click for mode details.",
            style: { cursor: "pointer" },
            events: { click: () => openKavachModeInfo(status.mode) },
          }, status.mode),
        ]),
        kv([
          ["Chain length", String(status.audit_chain.length)],
          ["Head hash", shortId(status.audit_chain.head_hash, 24)],
          ["Policy source", status.policy.source_path ?? "(inline in marg.toml)"],
          ["Policy hash", shortId(status.policy.source_hash, 24)],
          ["Loaded at", fmtIso(status.policy.loaded_at)],
          ["Policy rules", String(status.policy.rule_count)],
          ["Invariants", String(status.policy.invariant_count)],
          ["kavach-core", status.kavach_core_version],
          ["kavach-pq", status.kavach_pq_version],
        ]),
      );

      const total = status.audit_chain.length;
      const startFrom = Math.max(0, total - PAGE_LIMIT);
      const page = total > 0 ? await listKavachAuditEntries({ since: startFrom, limit: PAGE_LIMIT }, signal) : null;

      entriesBody.replaceChildren();
      if (!page || page.entries.length === 0) {
        entriesBody.appendChild(h("tr", {}, h("td", { colspan: 7, class: "empty" }, "Chain is empty.")));
        return;
      }
      for (const e of page.entries) {
        const data = (e.data ?? {}) as Record<string, unknown>;
        const schema = (data["schema"] as string) ?? "-";
        const verdict = data["verdict"] as Record<string, unknown> | undefined;
        const real = (verdict?.["real_kind"] as string) ?? "-";
        const eff = (verdict?.["effective_kind"] as string) ?? "-";
        const principal = (data["principal_id"] as string) ?? "-";
        const action = (data["action_name"] as string) ?? "-";
        const reasonCode = (verdict?.["reason_code"] as string | null) ?? null;
        const reasonText = (verdict?.["reason_text"] as string | null) ?? null;
        const reason = reasonCode ? `[${reasonCode}] ${reasonText ?? ""}` : "-";
        const verdictBadgeCls = real === "permit" ? "ok" : "err";
        entriesBody.appendChild(h("tr", {}, [
          h("td", {}, String(e.index)),
          h("td", { class: "mono" }, fmtIso(e.signed_payload_signed_at)),
          h("td", {}, h("code", {}, schema)),
          h("td", {}, [
            h("span", { class: `badge ${verdictBadgeCls}` }, real),
            h("span", { class: "mono", style: { marginLeft: "6px", fontSize: "10px", color: "var(--muted)" } }, `eff:${eff}`),
          ]),
          h("td", { class: "mono" }, shortId(principal, 16)),
          h("td", {}, action),
          h("td", {}, reason),
        ]));
      }
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Failed to load audit chain", e);
    }
  }

  await refresh();
}

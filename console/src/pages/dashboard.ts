import {
  fetchMetrics,
  getBudget,
  listKeys,
  listRequests,
  providerHealth,
  listAdminTokens,
} from "../api";
import { h, mount } from "../dom";
import { fmtIso, fmtMs, fmtNumber, fmtUsd, shortId } from "../format";
import { navigate } from "../router";
import { parseProm, sumWhere, topN } from "../prom";
import { toastError } from "../toast";
import { adminTokenStatus } from "../types";

const POLL_MS = 5000;

export async function renderDashboard(target: HTMLElement, signal: AbortSignal): Promise<void> {
  const reqsCard = kpi("Requests today", "-", "");
  const spendCard = kpi("Spend today", "-", "");
  const provsCard = kpi("Providers", "-", "");
  const tokensCard = kpi("Bootstrap status", "-", "");

  const spendersBody = h("tbody");
  const failoversBody = h("tbody");
  const errorsBody = h("tbody");

  const layout = h("div", { class: "page" }, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Dashboard"),
      h("div", { class: "controls" }, [
        h("span", { class: "mono", style: { fontSize: "11px", color: "var(--muted)" } }, `Polling ${POLL_MS / 1000}s`),
      ]),
    ]),
    h("div", { class: "cards" }, [reqsCard.el, spendCard.el, provsCard.el, tokensCard.el]),
    h("div", { class: "grid-2", style: { marginTop: "16px" } }, [
      h("div", { class: "card" }, [
        h("h3", { style: { marginTop: 0, fontSize: "14px" } }, "Top spenders today"),
        h("div", { class: "table-wrap" }, [
          h("table", {}, [
            h("thead", {}, h("tr", {}, [
              th("Key"),
              th("Principal"),
              th("Spent"),
              th("Remaining"),
            ])),
            spendersBody,
          ]),
        ]),
      ]),
      h("div", { class: "card" }, [
        h("h3", { style: { marginTop: 0, fontSize: "14px" } }, "Recent failovers"),
        h("div", { class: "table-wrap" }, [
          h("table", {}, [
            h("thead", {}, h("tr", {}, [
              th("From"),
              th("To"),
              th("Count"),
            ])),
            failoversBody,
          ]),
        ]),
      ]),
    ]),
    h("div", { class: "card", style: { marginTop: "16px" } }, [
      h("h3", { style: { marginTop: 0, fontSize: "14px" } }, "Last 20 errors (status >= 400)"),
      h("div", { class: "table-wrap" }, [
        h("table", {}, [
          h("thead", {}, h("tr", {}, [
            th("When"),
            th("Key"),
            th("Model"),
            th("Provider"),
            th("Status"),
            th("Latency"),
          ])),
          errorsBody,
        ]),
      ]),
    ]),
  ]);

  mount(target, layout);

  async function refresh(): Promise<void> {
    try {
      const [text, keys, prov, tokens, reqs] = await Promise.all([
        fetchMetrics(signal),
        listKeys({}, signal),
        providerHealth(signal),
        listAdminTokens(signal),
        listRequests({ limit: 20 }, signal),
      ]);
      const samples = parseProm(text);
      const reqs2xx = sumWhere(samples, "marg_requests_total", { status: (v) => v.startsWith("2") });
      const reqs4xx = sumWhere(samples, "marg_requests_total", { status: (v) => v.startsWith("4") });
      const reqs5xx = sumWhere(samples, "marg_requests_total", { status: (v) => v.startsWith("5") });
      const total = reqs2xx + reqs4xx + reqs5xx;
      reqsCard.value.textContent = fmtNumber(total);
      reqsCard.sub.textContent = `${fmtNumber(reqs4xx + reqs5xx)} errors`;

      const provNames = prov.providers.map((p) => p.name);
      provsCard.value.textContent = String(provNames.length);
      provsCard.sub.textContent = provNames.join(", ") || "none configured";

      const activeTokens = tokens.tokens.filter((t) => adminTokenStatus(t) === "active");
      const nonBootstrap = activeTokens.filter((t) => t.label && t.label !== "bootstrap");
      if (nonBootstrap.length > 0) {
        tokensCard.value.textContent = String(activeTokens.length);
        tokensCard.sub.textContent = "OK";
        tokensCard.el.classList.remove("warn");
      } else {
        tokensCard.value.textContent = String(activeTokens.length);
        tokensCard.sub.textContent = "only bootstrap token, rotate soon";
      }

      const activeKeys = keys.keys.filter((k) => k.status === "active");
      const cap = Math.min(activeKeys.length, 50);
      let spendToday = 0;
      const perKey: Array<{ keyId: string; prefix: string; principal: string; spent: number; remaining: number | null }> = [];
      for (let i = 0; i < cap; i++) {
        const k = activeKeys[i];
        try {
          const snap = await getBudget(k.id, signal);
          spendToday += snap.spent_usd;
          perKey.push({
            keyId: k.id,
            prefix: k.token_prefix ?? shortId(k.id, 10),
            principal: k.principal.id,
            spent: snap.spent_usd,
            remaining: snap.remaining_usd,
          });
        } catch (_e) {
          // skip on individual key error so the dashboard stays responsive
        }
      }
      spendCard.value.textContent = fmtUsd(spendToday);
      spendCard.sub.textContent = `${activeKeys.length} keys total${cap < activeKeys.length ? ` (showing top of first ${cap})` : ""}`;

      perKey.sort((a, b) => b.spent - a.spent);
      const top5 = perKey.slice(0, 5);
      spendersBody.replaceChildren();
      if (top5.length === 0) {
        spendersBody.appendChild(h("tr", {}, h("td", { colspan: 4, class: "empty" }, "No spend yet today.")));
      } else {
        for (const row of top5) {
          const tr = h("tr", {
            class: "clickable",
            events: { click: () => navigate(`/keys/${encodeURIComponent(row.keyId)}`) },
          }, [
            h("td", {}, h("code", {}, row.prefix)),
            h("td", {}, row.principal),
            h("td", {}, fmtUsd(row.spent)),
            h("td", {}, fmtUsd(row.remaining)),
          ]);
          spendersBody.appendChild(tr);
        }
      }

      const failoverGroups = topN(samples, "marg_failover_total", ["from_provider", "to_provider"], 5);
      failoversBody.replaceChildren();
      if (failoverGroups.length === 0) {
        failoversBody.appendChild(h("tr", {}, h("td", { colspan: 3, class: "empty" }, "No failovers recorded.")));
      } else {
        for (const g of failoverGroups) {
          failoversBody.appendChild(h("tr", {}, [
            h("td", {}, h("code", {}, g.labels["from_provider"] ?? "-")),
            h("td", {}, h("code", {}, g.labels["to_provider"] ?? "-")),
            h("td", {}, fmtNumber(g.value)),
          ]));
        }
      }

      const errors = reqs.entries.filter((r) => r.status >= 400);
      errorsBody.replaceChildren();
      if (errors.length === 0) {
        errorsBody.appendChild(h("tr", {}, h("td", { colspan: 6, class: "empty" }, "No recent errors.")));
      } else {
        for (const e of errors) {
          errorsBody.appendChild(h("tr", {
            class: "clickable",
            events: { click: () => navigate(`/requests?id=${encodeURIComponent(e.id)}`) },
          }, [
            h("td", { class: "mono" }, fmtIso(e.timestamp)),
            h("td", {}, h("code", {}, shortId(e.key_id, 10))),
            h("td", {}, e.model),
            h("td", {}, e.provider),
            h("td", {}, h("span", { class: "badge err" }, String(e.status))),
            h("td", {}, fmtMs(e.latency_ms)),
          ]));
        }
      }
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Dashboard refresh failed", e);
    }
  }

  await refresh();
  const timer = window.setInterval(() => {
    if (signal.aborted) {
      window.clearInterval(timer);
      return;
    }
    void refresh();
  }, POLL_MS);
  signal.addEventListener("abort", () => window.clearInterval(timer));
}

function th(label: string): HTMLElement {
  return h("th", {}, label);
}

function kpi(
  label: string,
  initial: string,
  sub: string,
): { el: HTMLElement; value: HTMLElement; sub: HTMLElement } {
  const value = h("div", { class: "value" }, initial);
  const subEl = h("div", { class: "sub" }, sub);
  const el = h("div", { class: "card kpi" }, [
    h("div", { class: "label" }, label),
    value,
    subEl,
  ]);
  return { el, value, sub: subEl };
}

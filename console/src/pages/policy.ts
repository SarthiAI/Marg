import { getPolicy, reloadPolicy } from "../api";
import { h, mount } from "../dom";
import { fmtIso, fmtUsd } from "../format";
import { toast, toastError } from "../toast";
import type { ConfigRoute, PersistedRoute, SplitEntry } from "../types";
import { kv, th } from "../ui";

export async function renderPolicy(target: HTMLElement, signal: AbortSignal): Promise<void> {
  const headerInfo = h("div", { class: "card" });
  const routesBody = h("tbody");
  const pricingBody = h("tbody");

  const layout = h("div", {}, [
    h("div", { class: "page-header" }, [
      h("h1", {}, "Policy"),
      h("div", { class: "controls" }, [
        h("button", { class: "ghost", events: { click: () => refresh() } }, "Refresh"),
        h("button", {
          class: "primary",
          events: {
            click: async () => {
              try {
                const r = await reloadPolicy(signal);
                toast(
                  `Reloaded: ${r.config_routes} config + ${r.stored_routes} stored routes, ${r.pricing_entries} pricing entries`,
                  "ok",
                );
                await refresh();
              } catch (e) {
                toastError("Reload failed", e);
              }
            },
          },
        }, "Reload policy"),
      ]),
    ]),
    headerInfo,
    h("h3", { style: { marginTop: "16px", fontSize: "14px" } }, "Effective routes"),
    h("p", { class: "help-block" }, "Config routes evaluate first (in declaration order), then persisted routes (by position)."),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Source"),
          th("Order"),
          th("Match"),
          th("Primary / split"),
          th("Fallbacks"),
        ])),
        routesBody,
      ]),
    ]),
    h("h3", { style: { marginTop: "24px", fontSize: "14px" } }, "Pricing table"),
    h("div", { class: "table-wrap" }, [
      h("table", {}, [
        h("thead", {}, h("tr", {}, [
          th("Model"),
          th("Input USD / 1K"),
          th("Output USD / 1K"),
        ])),
        pricingBody,
      ]),
    ]),
  ]);
  mount(target, layout);

  async function refresh(): Promise<void> {
    try {
      const pol = await getPolicy(signal);
      headerInfo.replaceChildren(
        h("h3", { style: { marginTop: 0, fontSize: "14px" } }, "Loaded from"),
        kv([
          ["Config path", pol.config_path],
          ["Default provider", pol.default_provider ?? "(first registered)"],
          ["Registered providers", pol.providers.join(", ") || "(none)"],
          ["Last refreshed", fmtIso(new Date().toISOString())],
        ]),
      );

      routesBody.replaceChildren();
      const allRows: Array<{ source: string; idx: number; row: HTMLElement }> = [];
      pol.config_routes.forEach((r, i) => {
        allRows.push({ source: "config", idx: i, row: routeRow("config", i + 1, r as ConfigRoute) });
      });
      pol.stored_routes.forEach((r, i) => {
        allRows.push({ source: "stored", idx: i, row: routeRow("stored", r.position ?? i + 1, r as PersistedRoute) });
      });
      if (allRows.length === 0) {
        routesBody.appendChild(h("tr", {}, h("td", { colspan: 5, class: "empty" }, "No routes configured.")));
      } else {
        for (const r of allRows) routesBody.appendChild(r.row);
      }

      pricingBody.replaceChildren();
      if (pol.pricing.length === 0) {
        pricingBody.appendChild(h("tr", {}, h("td", { colspan: 3, class: "empty" }, "Using built-in pricing defaults.")));
      } else {
        for (const p of pol.pricing) {
          pricingBody.appendChild(h("tr", {}, [
            h("td", {}, h("code", {}, p.model)),
            h("td", {}, fmtUsd(p.input_per_1k_usd)),
            h("td", {}, fmtUsd(p.output_per_1k_usd)),
          ]));
        }
      }
    } catch (e) {
      if ((e as { name?: string }).name === "AbortError") return;
      toastError("Failed to load policy", e);
    }
  }

  function routeRow(source: "config" | "stored", order: number, r: ConfigRoute | PersistedRoute): HTMLElement {
    const isStored = source === "stored";
    const matchModel = isStored ? (r as PersistedRoute).match_model : ((r as ConfigRoute).match?.model ?? null);
    const matchTeam = isStored ? (r as PersistedRoute).match_team : ((r as ConfigRoute).match?.team ?? null);
    const primary = isStored ? (r as PersistedRoute).primary : ((r as ConfigRoute).primary ?? null);
    const primaryModel = isStored ? (r as PersistedRoute).primary_model : null;
    const fallbacks = isStored ? ((r as PersistedRoute).fallbacks ?? []) : ((r as ConfigRoute).fallback ?? []);
    const split = isStored ? ((r as PersistedRoute).split ?? []) : ((r as ConfigRoute).split ?? []);
    return h("tr", {}, [
      h("td", {}, h("span", { class: `badge ${isStored ? "code" : "ok"}` }, source)),
      h("td", {}, String(order)),
      h("td", {}, matchSummary(matchModel ?? null, matchTeam ?? null)),
      h("td", {}, primarySummary(primary ?? null, primaryModel ?? null, split)),
      h("td", {}, fallbacks.join(", ") || "-"),
    ]);
  }

  await refresh();
}

function matchSummary(model: string | null, team: string | null): string {
  const parts: string[] = [];
  if (model) parts.push(`model=${model}`);
  if (team) parts.push(`team=${team}`);
  return parts.join(" ") || "(any)";
}

function primarySummary(
  primary: string | null,
  primaryModel: string | null,
  split: SplitEntry[],
): string {
  if (split.length > 0) {
    return `split: ${split.map((s) => `${s.provider}${s.model ? `:${s.model}` : ""}@${s.weight}`).join(", ")}`;
  }
  if (primary) {
    return `primary: ${primary}${primaryModel ? `:${primaryModel}` : ""}`;
  }
  return "-";
}

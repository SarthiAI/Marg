// Minimal Prometheus text-format parser. Enough for the dashboard. We only
// need counter and gauge metric values keyed by label sets; histograms are
// parsed flat (each bucket / sum / count looks like its own series here).

export interface PromSample {
  name: string;
  labels: Record<string, string>;
  value: number;
}

export function parseProm(text: string): PromSample[] {
  const out: PromSample[] = [];
  for (const rawLine of text.split("\n")) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;
    // name{label="value",label2="value"} 12345
    // or
    // name 12345
    const braceIdx = line.indexOf("{");
    let name: string;
    let labelsStr = "";
    let rest: string;
    if (braceIdx >= 0) {
      name = line.slice(0, braceIdx);
      const endBrace = line.indexOf("}", braceIdx);
      if (endBrace < 0) continue;
      labelsStr = line.slice(braceIdx + 1, endBrace);
      rest = line.slice(endBrace + 1).trim();
    } else {
      const spaceIdx = line.indexOf(" ");
      if (spaceIdx < 0) continue;
      name = line.slice(0, spaceIdx);
      rest = line.slice(spaceIdx + 1).trim();
    }
    const valueStr = rest.split(/\s+/)[0];
    const value = Number(valueStr);
    if (!isFinite(value)) continue;
    const labels: Record<string, string> = {};
    if (labelsStr) {
      // tolerant split: labels are `k="v",k="v"`. Quoted values may contain
      // commas, but Marg's metrics never emit such labels.
      const parts = labelsStr.split(",");
      for (const p of parts) {
        const eq = p.indexOf("=");
        if (eq < 0) continue;
        const k = p.slice(0, eq).trim();
        let v = p.slice(eq + 1).trim();
        if (v.startsWith(`"`) && v.endsWith(`"`)) v = v.slice(1, -1);
        labels[k] = v;
      }
    }
    out.push({ name, labels, value });
  }
  return out;
}

export function sumWhere(
  samples: PromSample[],
  name: string,
  matchers: Record<string, string | ((v: string) => boolean)> = {},
): number {
  let total = 0;
  for (const s of samples) {
    if (s.name !== name) continue;
    let ok = true;
    for (const [k, m] of Object.entries(matchers)) {
      const v = s.labels[k];
      if (typeof m === "function") {
        if (v === undefined || !m(v)) { ok = false; break; }
      } else if (v !== m) { ok = false; break; }
    }
    if (ok) total += s.value;
  }
  return total;
}

export function topN(
  samples: PromSample[],
  name: string,
  groupBy: string[],
  n: number,
): Array<{ labels: Record<string, string>; value: number }> {
  const grouped = new Map<string, { labels: Record<string, string>; value: number }>();
  for (const s of samples) {
    if (s.name !== name) continue;
    const labels: Record<string, string> = {};
    const key = groupBy.map((g) => {
      const v = s.labels[g] ?? "";
      labels[g] = v;
      return `${g}=${v}`;
    }).join("|");
    const prev = grouped.get(key);
    if (prev) prev.value += s.value;
    else grouped.set(key, { labels, value: s.value });
  }
  return [...grouped.values()]
    .sort((a, b) => b.value - a.value)
    .slice(0, n);
}

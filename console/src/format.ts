export function fmtUsd(amount: number | null | undefined): string {
  if (amount === null || amount === undefined) return "-";
  if (!isFinite(amount)) return "unlimited";
  if (amount === 0) return "$0.00";
  if (amount < 0.01) return `$${amount.toFixed(4)}`;
  return `$${amount.toFixed(2)}`;
}

export function fmtNumber(n: number | null | undefined): string {
  if (n === null || n === undefined) return "-";
  return n.toLocaleString("en-US");
}

export function fmtMs(n: number | null | undefined): string {
  if (n === null || n === undefined) return "-";
  if (n < 1) return "<1ms";
  if (n < 1000) return `${Math.round(n)}ms`;
  return `${(n / 1000).toFixed(2)}s`;
}

export function fmtRelative(iso: string | null | undefined): string {
  if (!iso) return "-";
  const t = new Date(iso).getTime();
  if (isNaN(t)) return iso;
  const diff = Date.now() - t;
  const abs = Math.abs(diff);
  const past = diff >= 0;
  if (abs < 5_000) return "just now";
  if (abs < 60_000) return past ? `${Math.floor(abs / 1000)}s ago` : `in ${Math.floor(abs / 1000)}s`;
  if (abs < 3_600_000) return past ? `${Math.floor(abs / 60_000)}m ago` : `in ${Math.floor(abs / 60_000)}m`;
  if (abs < 86_400_000) return past ? `${Math.floor(abs / 3_600_000)}h ago` : `in ${Math.floor(abs / 3_600_000)}h`;
  return new Date(iso).toISOString().slice(0, 19).replace("T", " ");
}

export function fmtIso(iso: string | null | undefined): string {
  if (!iso) return "-";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toISOString().slice(0, 19).replace("T", " ") + "Z";
}

export function shortId(id: string | null | undefined, len = 8): string {
  if (!id) return "-";
  if (id.length <= len) return id;
  return `${id.slice(0, len)}...`;
}

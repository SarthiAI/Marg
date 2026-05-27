// Shapes the admin API returns. Hand-mirrored from /admin/openapi.json so
// the console stays terse and dependency-free.

export interface Principal {
  id: string;
  kind: "user" | "service" | "agent";
}

export interface MargKey {
  id: string;
  token_hash?: string;
  token_prefix?: string;
  principal: Principal;
  team?: string | null;
  status: "active" | "revoked";
  created_at: string;
  revoked_at?: string | null;
}

export interface BudgetSpec {
  key_id: string;
  daily_usd: number;
  rpm: number;
}

export interface BudgetSnapshot {
  budget: BudgetSpec | null;
  day: string;
  spent_usd: number;
  // null when the daily cap is 0 (unlimited): serde turns f64::INFINITY into
  // JSON null. Treat null as "unlimited" in the UI.
  remaining_usd: number | null;
}

export interface SplitEntry {
  provider: string;
  weight: number;
  model?: string | null;
}

export interface RouteSpec {
  position?: number | null;
  match_model?: string | null;
  match_team?: string | null;
  primary?: string | null;
  primary_model?: string | null;
  fallbacks?: string[];
  split?: SplitEntry[];
}

export interface PersistedRoute extends RouteSpec {
  id: string;
  created_at: string;
}

export interface ConfigRoute {
  match?: { model?: string | null; team?: string | null };
  primary?: string | null;
  fallback?: string[];
  split?: SplitEntry[];
}

export interface PricingEntry {
  model: string;
  input_per_1k_usd: number;
  output_per_1k_usd: number;
}

export interface KavachDriftDetectorView {
  name: string;
  parameters: Record<string, unknown>;
}

export interface KavachDriftView {
  enabled: boolean;
  warning_threshold: number;
  detectors: KavachDriftDetectorView[];
}

export interface KavachPermitSignerView {
  enabled: boolean;
  algorithm: string;
  key_id: string;
}

export interface KavachPolicyView {
  mode: "observe" | "enforce" | string;
  policy_path: string | null;
  policy_source_hash: string;
  loaded_at: string;
  policy_rule_count: number;
  invariant_count: number;
  audit_chain_length: number;
  audit_chain_head_hash: string;
  core_version: string;
  pq_version: string;
  permit_signer: KavachPermitSignerView;
  drift: KavachDriftView;
}

export interface PolicyView {
  config_path: string;
  providers: string[];
  default_provider: string | null;
  config_routes: ConfigRoute[];
  stored_routes: PersistedRoute[];
  pricing: PricingEntry[];
  kavach: KavachPolicyView;
}

export interface KavachAuditStatus {
  mode: string;
  kavach_core_version: string;
  kavach_pq_version: string;
  audit_chain: { head_hash: string; length: number };
  policy: {
    source_path: string | null;
    source_hash: string;
    loaded_at: string;
    rule_count: number;
    invariant_count: number;
  };
  permits: {
    expose_to_caller: boolean;
    forward_to_provider: boolean;
    ttl_seconds: number;
    signer: KavachPermitSignerView;
  };
  drift: KavachDriftView;
}

export interface KavachAuditEntryView {
  index: number;
  previous_hash: string;
  entry_hash: string;
  mode: string;
  signed_payload_key_id: string;
  signed_payload_signed_at: string;
  data: Record<string, unknown> | null;
}

export interface KavachAuditEntriesResponse {
  head_hash: string;
  total: number;
  from: number;
  count: number;
  entries: KavachAuditEntryView[];
}

export interface KavachVerifyResponse {
  verified: boolean;
  source: string;
  count: number;
  error?: string;
}

export interface ProviderHealth {
  name: string;
  configured: boolean;
  successes_total: number;
  errors_5xx: number;
  errors_4xx: number;
  timeouts: number;
  network_errors: number;
}

export interface RouteAttempt {
  provider: string;
  model: string;
  status: number;
  latency_ms: number;
  outcome: string;
  error?: string | null;
}

export interface RequestLogEntry {
  id: string;
  timestamp: string;
  key_id: string;
  principal_id: string;
  provider: string;
  model: string;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  latency_ms: number;
  status: number;
  stream: boolean;
  error?: string | null;
  attempts: RouteAttempt[];
}

export interface AdminTokenRecord {
  id: string;
  token_prefix: string;
  label: string;
  created_at: string;
  revoked_at?: string | null;
  last_used_at?: string | null;
}

export function adminTokenStatus(t: AdminTokenRecord): "active" | "revoked" {
  return t.revoked_at ? "revoked" : "active";
}

export interface ApiErrorBody {
  error: { code: string; message: string };
}

export class ApiError extends Error {
  code: string;
  status: number;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export type SessionId = "refactor" | "explain" | "tests" | "review";
export type CodingStage =
  | "ready"
  | "answered"
  | "checkpointed"
  | "complete"
  | "failed";
export type RepositoryStage =
  | "ready"
  | "proposed"
  | "parked"
  | "helper_done"
  | "complete"
  | "failed";
export type ApiResult<T> = T & { error?: string };
export interface EngineState<T> {
  configured: boolean;
  busy: boolean;
  report: T | null;
}
export interface SessionSpec {
  id: SessionId;
  label: string;
  estimated_units: number;
  rebuild_ms: number;
}
export interface SessionView {
  spec: SessionSpec;
  residency: "ram" | "disk";
  checkpoint_bytes: number;
  turns: number;
}
export interface Outcome {
  access: "cold" | "ram_hit" | "disk_restore";
  session: SessionId;
  evicted: SessionId[];
  modeled_ms: number;
}
export interface PolicyView {
  sessions: SessionView[];
  used_units: number;
  stats: {
    ram_hits: number;
    disk_restores: number;
    modeled_ms: number;
    requests: number;
  };
}
export interface Turn {
  index: number;
  task: { spec: SessionSpec; prompt: string; response: string };
  outcomes: [Outcome, Outcome];
}
export interface Snapshot {
  mode: string;
  step: number;
  total: number;
  budget: number;
  policies: [PolicyView, PolicyView];
  current: Turn | null;
  history: Turn[];
  checkpoint_files: number;
  checkpoint_bytes: number;
}
export interface Identity {
  engine_build: string;
  context_tokens: number;
  model_sha256: string;
}
export interface Observation<T> {
  elapsed_ms: number;
  engine: T;
}
export interface Timings {
  cache_n: number;
  prompt_n: number;
  prompt_ms: number;
  predicted_n: number;
  predicted_ms: number;
}
export interface Generation {
  content: string;
  stop_type: string;
  tokens: number[];
  timings: Timings;
}
export interface Prefill {
  timings: Pick<Timings, "cache_n" | "prompt_n" | "prompt_ms">;
}
export interface Manifest {
  bytes: number;
  saved_tokens: number;
  filename: string;
}
export interface RestoredSlot {
  n_restored: number;
  n_read: number;
}
export interface ProbeReport {
  mode: string;
  verified: boolean;
  identity: Identity;
  prompt_tokens: number;
  continuation_tokens: number;
  cold_prefill: Observation<Prefill>;
  save: Observation<Manifest>;
  restore: Observation<RestoredSlot>;
  reused_prefill: Observation<Prefill>;
  continued_prefill: Observation<Prefill>;
}
export interface CodingReport {
  stage: CodingStage;
  original: string;
  followup_reference: string;
  output_limit: number;
  identity: Identity | null;
  first: Observation<Generation> | null;
  followup: Observation<Generation> | null;
  validation: {
    accepted: boolean;
    passed: boolean;
    cases: number;
    details: string;
  } | null;
  save: Observation<Manifest> | null;
  restore: Observation<RestoredSlot> | null;
  slot_cleared: boolean;
  canonical_prefix_tokens: number;
  boundary_tokens_omitted: number;
}
export interface ProjectCheck {
  accepted: boolean;
  passed: boolean;
  output: string;
  patch: string;
}
export interface RepositoryReport {
  stage: RepositoryStage;
  context: { path: string; sha256: string; excerpt: string };
  proposal: Observation<Generation> | null;
  revision: Observation<Generation> | null;
  helper: Observation<Generation> | null;
  initial_check: ProjectCheck | null;
  final_check: ProjectCheck | null;
  checkpoint: Observation<Manifest> | null;
  restore: Observation<RestoredSlot> | null;
}

export type Viewport = {
  width: number;
  height: number;
  label?: string;
  deviceScaleFactor?: number;
};

export type Persona = {
  id: string;
  label: string;
  auth: { kind: "anonymous" } | { kind: "storage-state"; path: string };
  login?: {
    path?: string;
    successUrl: string;
  };
};

export type ReadyCondition =
  | { kind: "selector"; selector: string; timeoutMs?: number }
  | { kind: "response"; urlPattern: string; status?: number; timeoutMs?: number }
  | { kind: "network-idle"; timeoutMs?: number };

export type Interaction =
  | { action: "click"; selector: string; timeoutMs?: number }
  | { action: "fill"; selector: string; value: string; timeoutMs?: number }
  | { action: "press"; selector: string; key: string; timeoutMs?: number }
  | { action: "wait"; ready: ReadyCondition };

export type CapturePoint = {
  id: string;
  label: string;
  interaction?: Interaction[];
  ready?: ReadyCondition;
  fullPage?: boolean;
};

export type RouteScenario = {
  id: string;
  label: string;
  path: string;
  goal: string;
  dataState: string;
  ready: ReadyCondition;
  capturePoints: CapturePoint[];
};

export type OwnedProcess = {
  id: string;
  command: string;
  args?: string[];
  cwd?: string;
  env?: Record<string, string>;
  readyUrl?: string;
  readyTimeoutMs?: number;
};

export type Scenario = {
  schemaVersion: 1;
  id: string;
  title: string;
  baseUrl?: string;
  locale?: string;
  timezone?: string;
  colorScheme?: "light" | "dark";
  reducedMotion?: "reduce" | "no-preference";
  redact?: {
    selectors?: string[];
    text?: string[];
  };
  personas: Persona[];
  viewports: Viewport[];
  routes: RouteScenario[];
  processes?: OwnedProcess[];
};

export type CaptureError = {
  kind: "console" | "page" | "request" | "document" | "tool";
  message: string;
  url?: string;
  status?: number;
};

export type ScreenshotEvidence = {
  kind: "viewport" | "full-page";
  path: string;
  sha256: string;
};

export type CaptureEvidence = {
  persona: { id: string; label: string };
  route: { id: string; path: string; goal: string; dataState: string };
  viewport: Viewport;
  theme: string;
  capturePoint: { id: string; label: string };
  document: { url: string; status: number | null };
  screenshots: ScreenshotEvidence[];
  snapshotPath: string | null;
  errors: CaptureError[];
  startedAt: string;
  finishedAt: string;
};

export type ReviewContext = {
  schemaVersion: 1;
  runId: string;
  scenario: { id: string; title: string; sourcePath: string };
  source: { revision: string | null; dirty: boolean | null };
  baseUrl: string;
  browser: { name: "chromium"; version: string };
  createdAt: string;
  status: "completed" | "completed-with-errors" | "failed";
  filters: { personas: string[]; routes: string[]; viewports: string[] };
  captures: CaptureEvidence[];
  contactSheet: { html: string | null; png: string | null };
  diagnostics: CaptureError[];
};

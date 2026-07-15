export type WorkspaceAlertLevel = "success" | "info" | "warning" | "error" | "system" | "debug";

export type WorkspaceAlert = {
  id: string;
  level: WorkspaceAlertLevel;
  title?: string;
  message: string;
  createdAt: number;
};

type AlertSubscriber = (alerts: WorkspaceAlert[]) => void;

let sequence = 0;
let alerts: WorkspaceAlert[] = [];
const subscribers = new Set<AlertSubscriber>();

function emit(): void {
  const snapshot = alerts.slice();
  for (const subscriber of subscribers) subscriber(snapshot);
}

export const workspaceAlerts = {
  subscribe(subscriber: AlertSubscriber): () => void {
    subscribers.add(subscriber);
    subscriber(alerts.slice());
    return () => subscribers.delete(subscriber);
  },
};

export function pushWorkspaceAlert(
  level: WorkspaceAlertLevel,
  message: string,
  options: { title?: string; id?: string } = {},
): string {
  const id = options.id ?? `${Date.now().toString(36)}-${(sequence++).toString(36)}`;
  alerts = [
    ...alerts.filter((alert) => alert.id !== id),
    {
      id,
      level,
      title: options.title,
      message,
      createdAt: Date.now(),
    },
  ].slice(-8);
  emit();
  return id;
}

export function dismissWorkspaceAlert(id: string): void {
  const next = alerts.filter((alert) => alert.id !== id);
  if (next.length === alerts.length) return;
  alerts = next;
  emit();
}

export function clearWorkspaceAlerts(): void {
  if (alerts.length === 0) return;
  alerts = [];
  emit();
}

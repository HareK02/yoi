export type ApiResult<T> = {
  data: T | null;
  error: string | null;
};

function normalizePath(path: string): string {
  if (!path || path === '/') return '';
  return path.startsWith('/') ? path : `/${path}`;
}

export function workspaceRoute(workspaceId: string, path = ''): string {
  return `/w/${encodeURIComponent(workspaceId)}${normalizePath(path)}`;
}

export function workspaceApiPath(workspaceId: string, path = ''): string {
  return `/api/w/${encodeURIComponent(workspaceId)}${normalizePath(path)}`;
}

export async function loadJson<T>(
  fetchFn: typeof fetch,
  path: string,
): Promise<ApiResult<T>> {
  try {
    const response = await fetchFn(path);
    if (!response.ok) {
      const text = await response.text();
      return {
        data: null,
        error: text || `${path} request failed (${response.status})`,
      };
    }
    return { data: (await response.json()) as T, error: null };
  } catch (error) {
    return {
      data: null,
      error: error instanceof Error ? error.message : `${path} request failed`,
    };
  }
}

export function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString();
}

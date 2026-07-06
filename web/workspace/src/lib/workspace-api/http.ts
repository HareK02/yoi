export type ApiResult<T> = {
  data: T | null;
  error: string | null;
};

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

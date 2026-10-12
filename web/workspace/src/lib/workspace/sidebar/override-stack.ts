/** Header／Sidebarのlayout登録を新しい順に表示するstack。
 * cleanupは登録固有のIDだけを除き、重複cleanupや古いlayoutの破棄で新しい内容を消さない。
 * active登録がなくなればnullを通知し、呼出側のfallbackへ戻す。
 */
export type OverrideDisposer = () => void;

export type OverrideStack<T> = {
  register(value: T): OverrideDisposer;
};

export function createOverrideStack<T>(
  setActive: (value: T | null) => void,
): OverrideStack<T> {
  let entries: Array<{ id: symbol; value: T }> = [];

  return {
    register(value: T): OverrideDisposer {
      const id = Symbol();
      let disposed = false;

      entries = [...entries, { id, value }];
      setActive(value);

      return () => {
        if (disposed) return;
        disposed = true;
        entries = entries.filter((entry) => entry.id !== id);
        setActive(entries.at(-1)?.value ?? null);
      };
    },
  };
}

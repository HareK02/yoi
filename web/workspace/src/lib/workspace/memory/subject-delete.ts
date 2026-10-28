// 削除開始後はGETから対象が消えるため、同じtabでの再読込みにも依頼IDを保持する。
// Subject本文は保存しない。これは削除結果の正本ではなく、安全な再送用の最小入力。
export type SubjectDeleteAttempt = {
  subjectId: string;
  operationId: string;
};
export function subjectDeleteStorageKey(
  workspaceId: string,
  subjectId: string,
): string {
  return `yoi.subject-delete:${JSON.stringify([workspaceId, subjectId])}`;
}
export function readSubjectDeleteAttempt(
  storage: Pick<Storage, "getItem">,
  workspaceId: string,
  subjectId: string,
): SubjectDeleteAttempt | null {
  const raw = storage.getItem(subjectDeleteStorageKey(workspaceId, subjectId));
  if (raw === null) return null;
  const value = JSON.parse(raw);
  if (
    !value || value.subjectId !== subjectId ||
    typeof value.operationId !== "string" ||
    value.operationId.length === 0 ||
    Object.keys(value).some((key) =>
      key !== "subjectId" && key !== "operationId"
    )
  ) {
    throw new Error(
      "The saved deletion request cannot be read. Do not start another deletion request.",
    );
  }
  return value;
}
export function saveSubjectDeleteAttempt(
  storage: Pick<Storage, "setItem">,
  workspaceId: string,
  attempt: SubjectDeleteAttempt,
): void {
  storage.setItem(
    subjectDeleteStorageKey(workspaceId, attempt.subjectId),
    JSON.stringify(attempt),
  );
}
export function clearSubjectDeleteAttempt(
  storage: Pick<Storage, "removeItem">,
  workspaceId: string,
  subjectId: string,
): void {
  storage.removeItem(subjectDeleteStorageKey(workspaceId, subjectId));
}

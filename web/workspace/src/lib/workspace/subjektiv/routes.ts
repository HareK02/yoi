import { workspaceRoute } from "#lib/workspace/api/http.ts";

export function subjectsHref(workspaceId: string): string {
  return workspaceRoute(workspaceId, "/subjektiv");
}

export function subjectHref(workspaceId: string, subjectId: string): string {
  return `${subjectsHref(workspaceId)}/${encodeURIComponent(subjectId)}`;
}

export function subjectPermissionsHref(
  workspaceId: string,
  subjectId: string,
): string {
  return `${subjectHref(workspaceId, subjectId)}/permissions`;
}

export function subjectJournalHref(
  workspaceId: string,
  subjectId: string,
): string {
  return `${subjectHref(workspaceId, subjectId)}/journal`;
}

export function subjectChatHref(
  workspaceId: string,
  subjectId: string,
): string {
  return `${subjectHref(workspaceId, subjectId)}/chat`;
}

export function subjectMemoryHref(
  workspaceId: string,
  subjectId: string,
  memoryId: string,
): string {
  return `${subjectHref(workspaceId, subjectId)}/memories/${
    encodeURIComponent(memoryId)
  }`;
}

/** 旧SPAの静的予約語を優先する。Memory IDを新しい画面の予約語と競合させない。
 * pathnameを分割してからdecodeするため、ID内のencoded slashを階層に読み替えない。
 * query/hashは解析・再生成せず引き継ぐ。未知の階層から移行先を推測しない。
 */
export function legacySubjectDestination(
  url: Pick<URL, "pathname" | "search" | "hash">,
): string | null {
  const segments = url.pathname.split("/").slice(1);
  if (segments.at(-1) === "") segments.pop();
  if (segments[0] !== "w" || !segments[1] || segments[2] !== "memory") {
    return null;
  }
  let decoded: string[];
  try {
    decoded = segments.map(decodeURIComponent);
  } catch {
    return null;
  }
  const workspaceId = decoded[1];
  const subjectId = decoded[3];
  let destination: string;
  if (decoded.length === 3) destination = subjectsHref(workspaceId);
  // stagingは旧一覧と同階層の完全一致だけを所有し、その子routeは動的Subjectだった。
  else if (!subjectId || (subjectId === "staging" && decoded.length === 4)) {
    return null;
  } else if (decoded.length === 4) {
    destination = subjectHref(workspaceId, subjectId);
  } else if (decoded.length === 5 && decoded[4]) {
    switch (decoded[4]) {
      case "permissions":
        destination = subjectPermissionsHref(workspaceId, subjectId);
        break;
      case "journal":
        destination = subjectJournalHref(workspaceId, subjectId);
        break;
      case "conversation":
      case "chat":
        destination = subjectChatHref(workspaceId, subjectId);
        break;
      default:
        destination = subjectMemoryHref(workspaceId, subjectId, decoded[4]);
    }
  } else return null;
  return destination + url.search + url.hash;
}

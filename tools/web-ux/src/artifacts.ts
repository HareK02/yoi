import { dirname, relative, resolve } from "@std/path";

const SECRET_PATTERNS: RegExp[] = [
  /\b(authorization|cookie|set-cookie|x-csrf-token)\b\s*[:=]\s*[^\s,;]+/gi,
  /\b(bearer)\s+[a-z0-9._~+\/-]+=*/gi,
  /\b(session|token|credential|password|passkey|private[_ -]?key)\b\s*[:=]\s*["']?[^\s,"'};]+/gi,
];

export function redactText(value: string, exactSecrets: string[] = []): string {
  let result = value;
  for (const secret of exactSecrets) {
    if (secret) result = result.replaceAll(secret, "[REDACTED]");
  }
  for (const pattern of SECRET_PATTERNS) result = result.replaceAll(pattern, "$1=[REDACTED]");
  return result;
}

export function bounded(value: string, maximum = 1000): string {
  const normalized = value.replaceAll(/\s+/g, " ").trim();
  return normalized.length <= maximum ? normalized : `${normalized.slice(0, maximum - 1)}…`;
}

export function safeUrl(value: string, baseUrl?: string): string {
  try {
    const url = new URL(value, baseUrl);
    url.username = "";
    url.password = "";
    for (const key of [...url.searchParams.keys()]) url.searchParams.set(key, "[REDACTED]");
    url.hash = "";
    return url.toString();
  } catch {
    return "[invalid-url]";
  }
}

export function assertBundleIsSecretFree(serialized: string, exactSecrets: string[] = []): void {
  const lower = serialized.toLowerCase();
  for (const forbidden of ["authorization:", "set-cookie:", "cookie:", "bearer "]) {
    if (lower.includes(forbidden)) {
      throw new Error(`review bundle contains forbidden secret marker: ${forbidden}`);
    }
  }
  for (const secret of exactSecrets) {
    if (secret && serialized.includes(secret)) {
      throw new Error("review bundle contains configured secret text");
    }
  }
}

export async function ensurePrivateDirectory(path: string): Promise<void> {
  await Deno.mkdir(path, { recursive: true, mode: 0o700 });
  if (Deno.build.os !== "windows") await Deno.chmod(path, 0o700);
}

export async function makePrivate(path: string): Promise<void> {
  if (Deno.build.os !== "windows") await Deno.chmod(path, 0o600);
}

export async function writePrivateJson(path: string, value: unknown): Promise<void> {
  await ensurePrivateDirectory(dirname(path));
  await Deno.writeTextFile(path, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  if (Deno.build.os !== "windows") await Deno.chmod(path, 0o600);
}

export function workdirLogicalPath(repositoryRoot: string, path: string): string | null {
  const absolute = resolve(path);
  const logical = relative(repositoryRoot, absolute);
  if (logical === "" || (!logical.startsWith("..") && !logical.startsWith("/"))) {
    return logical || ".";
  }
  return null;
}

const TEXT_ARTIFACT_EXTENSIONS = [".json", ".md", ".html", ".log", ".txt"];

async function artifactFiles(root: string): Promise<string[]> {
  const files: string[] = [];
  const visit = async (directory: string) => {
    for await (const entry of Deno.readDir(directory)) {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory) await visit(path);
      else if (entry.isFile) files.push(path);
    }
  };
  await visit(root);
  return files;
}

export async function assertReviewBundleIsSecretFree(
  root: string,
  exactSecrets: string[] = [],
): Promise<void> {
  const secrets = exactSecrets.filter(Boolean);
  const overlapLength = Math.max(256, ...secrets.map((secret) => secret.length + 1));
  for (const path of await artifactFiles(root)) {
    const isText = TEXT_ARTIFACT_EXTENSIONS.some((extension) => path.endsWith(extension));
    const file = await Deno.open(path, { read: true });
    const decoder = new TextDecoder();
    let overlap = "";
    try {
      const buffer = new Uint8Array(64 * 1024);
      while (true) {
        const count = await file.read(buffer);
        if (count === null) break;
        const content = overlap + decoder.decode(buffer.subarray(0, count), { stream: true });
        for (const secret of secrets) {
          if (content.includes(secret)) {
            throw new Error(
              `review bundle artifact contains configured secret text: ${relative(root, path)}`,
            );
          }
        }
        if (isText) assertBundleIsSecretFree(content, secrets);
        overlap = content.slice(-overlapLength);
      }
      const final = overlap + decoder.decode();
      for (const secret of secrets) {
        if (final.includes(secret)) {
          throw new Error(
            `review bundle artifact contains configured secret text: ${relative(root, path)}`,
          );
        }
      }
      if (isText) assertBundleIsSecretFree(final, secrets);
    } finally {
      file.close();
    }
  }
}

export async function sha256File(path: string): Promise<string> {
  const bytes = await Deno.readFile(path);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

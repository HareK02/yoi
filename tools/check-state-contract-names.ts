// The sole spelling here defines the prohibited vocabulary, not a state model.
export const forbiddenWord = "revision";

export type ExceptionKind = "saved_data" | "rejection_test" | "naming_policy";
export interface ExceptionRule {
  path: string;
  kind: ExceptionKind;
  reason: string;
  lines: { sha256: string; count: number }[];
}
export interface Finding {
  path: string;
  line: number;
  text: string;
  sha256: string;
}

export async function digest(text: string): Promise<string> {
  const bytes = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(text),
  );
  return Array.from(
    new Uint8Array(bytes),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
}

export async function findNames(
  path: string,
  content: string,
): Promise<Finding[]> {
  const pattern = new RegExp(forbiddenWord, "i");
  const findings: Finding[] = [];
  if (pattern.test(path)) {
    findings.push({ path, line: 0, text: path, sha256: await digest(path) });
  }
  const lines = content.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    if (pattern.test(lines[i])) {
      findings.push({
        path,
        line: i + 1,
        text: lines[i],
        sha256: await digest(lines[i].trim()),
      });
    }
  }
  return findings;
}

export function checkFindings(findings: Finding[], rules: ExceptionRule[]) {
  const remaining = new Map<string, number>();
  for (const rule of rules) {
    if (
      !rule.reason.trim() ||
      !["saved_data", "rejection_test", "naming_policy"].includes(rule.kind)
    ) {
      throw new Error(`Missing classified rationale for ${rule.path}`);
    }
    for (const line of rule.lines) {
      const key = `${rule.path}\0${line.sha256}`;
      if (
        remaining.has(key) || !Number.isSafeInteger(line.count) ||
        line.count < 1
      ) {
        throw new Error(`Invalid or duplicate exception in ${rule.path}`);
      }
      remaining.set(key, line.count);
    }
  }
  const unexpected: Finding[] = [];
  for (const finding of findings) {
    const key = `${finding.path}\0${finding.sha256}`;
    const count = remaining.get(key) ?? 0;
    if (count === 0) unexpected.push(finding);
    else remaining.set(key, count - 1);
  }
  const stale = [...remaining].filter(([, count]) => count !== 0).map(([key]) =>
    key.replace("\0", ":")
  );
  return { unexpected, stale };
}

if (import.meta.main) {
  const result = await new Deno.Command("git", {
    args: ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
    stdout: "piped",
    stderr: "piped",
  }).output();
  if (!result.success) throw new Error(new TextDecoder().decode(result.stderr));
  const paths = [
    ...new Set(
      new TextDecoder().decode(result.stdout).split("\0").filter(Boolean),
    ),
  ].sort();
  const findings: Finding[] = [];
  for (const path of paths) {
    let bytes: Uint8Array;
    try {
      bytes = await Deno.readFile(path);
    } catch (error) {
      if (error instanceof Deno.errors.NotFound) continue;
      throw error;
    }
    // Include generated binaries in the audit; decoding is only for finding ASCII
    // names, never for rewriting data. No directory or extension is exempted.
    findings.push(...await findNames(path, new TextDecoder().decode(bytes)));
  }
  const rules: ExceptionRule[] = JSON.parse(
    await Deno.readTextFile("tools/state-contract-name-exceptions.json"),
  );
  const { unexpected, stale } = checkFindings(findings, rules);
  if (Deno.args.includes("--report")) {
    for (const finding of findings) {
      console.log(`${finding.path}:${finding.line}: ${finding.text}`);
    }
    for (const rule of rules) {
      console.log(`[${rule.kind}] ${rule.path}: ${rule.reason}`);
    }
  }
  for (const finding of unexpected) {
    console.error(
      `UNAPPROVED ${finding.path}:${finding.line}: ${finding.text}`,
    );
  }
  for (const key of stale) console.error(`STALE EXCEPTION ${key}`);
  console.log(
    `${findings.length} matching lines; ${unexpected.length} unapproved; ${stale.length} stale exceptions`,
  );
  Deno.exit(unexpected.length || stale.length ? 1 : 0);
}

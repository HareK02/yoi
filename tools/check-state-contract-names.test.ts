import {
  checkFindings,
  digest,
  type ExceptionRule,
  findNames,
  forbiddenWord,
} from "./check-state-contract-names.ts";
function assert(condition: boolean, message: string) {
  if (!condition) throw new Error(message);
}
const spelling = `${forbiddenWord.toUpperCase()}Conflict`;

Deno.test("case and compound names are rejected in content and file names", async () => {
  const found = await findNames(
    `src/${spelling}.rs`,
    `enum Error { ${spelling} }`,
  );
  assert(found.length === 2, "file name and content must both be inspected");
  assert(
    checkFindings(found, []).unexpected.length === 2,
    "both uses need review",
  );
});
Deno.test("saved-data exceptions match an exact line in exactly one file", async () => {
  const line = `SELECT old_${forbiddenWord} FROM archived_rows;`;
  const rules: ExceptionRule[] = [{
    path: "migration.sql",
    kind: "saved_data",
    reason: "Read the persisted old column before dropping it",
    lines: [{ sha256: await digest(line), count: 1 }],
  }];
  assert(
    checkFindings(await findNames("migration.sql", line), rules).unexpected
      .length === 0,
    "reviewed boundary should pass",
  );
  const copied = checkFindings(await findNames("current.sql", line), rules);
  assert(
    copied.unexpected.length === 1 && copied.stale.length === 1,
    "copying an allowed line elsewhere must fail",
  );
  const changed = checkFindings(
    await findNames("migration.sql", `${line} -- changed`),
    rules,
  );
  assert(
    changed.unexpected.length === 1 && changed.stale.length === 1,
    "changed boundary requires another review",
  );
});
Deno.test("duplicate uses cannot borrow the same exception and deleted uses leave stale exceptions", async () => {
  const line = `old_${forbiddenWord}`;
  const rules: ExceptionRule[] = [{
    path: "wire.rs",
    kind: "saved_data",
    reason: "Deserialize a saved key",
    lines: [{ sha256: await digest(line), count: 1 }],
  }];
  assert(
    checkFindings(await findNames("wire.rs", `${line}\n${line}`), rules)
      .unexpected.length === 1,
    "extra copy must fail",
  );
  assert(
    checkFindings([], rules).stale.length === 1,
    "unused exceptions must be removed",
  );
});
Deno.test("exceptions require a classified rationale", () => {
  let rejected = false;
  try {
    checkFindings([], [{
      path: "x",
      kind: "saved_data",
      reason: "",
      lines: [],
    }]);
  } catch {
    rejected = true;
  }
  assert(rejected, "a blank waiver cannot be accepted");
});
Deno.test("generated binary bytes are not excluded from name detection", async () => {
  const binary = new TextEncoder().encode(`\0asm\0${spelling}\0`);
  assert(
    (await findNames("module.wasm", new TextDecoder().decode(binary)))
      .length === 1,
    "binary names must be audited too",
  );
});

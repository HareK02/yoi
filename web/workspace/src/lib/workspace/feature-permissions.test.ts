import assignmentFixture from "../../../../../crates/feature-permissions/tests/fixtures/assignment.json" with {
  type: "json",
};
import catalogFixture from "../../../../../crates/feature-permissions/tests/fixtures/catalog.json" with {
  type: "json",
};
import type {
  NamespaceCatalog,
  PermissionAssignment,
  PermissionReference,
} from "../generated/feature-permissions.generated.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

// Rustが同じfixtureを本番Serdeで検証する。ここではWebが利用する公開形と
// children/多段キーの保持だけを確認し、認可や入力validatorの代用にしない。
function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, received ${
        JSON.stringify(actual)
      }`,
    );
  }
}

Deno.test("shared assignment fixture preserves Set/Namespace tags and full keys in the Web wire shape", () => {
  const expected: PermissionAssignment = {
    grants: [
      { kind: "set", value: "ticket" },
      {
        kind: "namespace",
        value: {
          namespace: "ticket",
          keys: ["ticket.hoge", "ticket.hoge.fuga.finish"],
        },
      },
    ],
    exclusions: [{
      kind: "namespace",
      value: { namespace: "ticket", keys: ["ticket.hoge.unlinked"] },
    }],
    revoked: false,
  };
  assertEquals(assignmentFixture, expected);
  assertEquals(JSON.parse(JSON.stringify(assignmentFixture)), expected);
});

Deno.test("shared NamespaceCatalog fixture preserves multilevel children without deriving prefix edges", () => {
  const namespace = catalogFixture.namespaces[0]!;
  assertEquals(namespace.namespace, "ticket");
  assertEquals(namespace.feature, "builtin:ticket");
  assertEquals(namespace.enabled, true);
  type CatalogPermission =
    NamespaceCatalog["declaration"]["permissions"][number];
  const expectedEdges: Pick<CatalogPermission, "key" | "children">[] = [
    { key: "ticket.hoge", children: ["ticket.hoge.fuga"] },
    { key: "ticket.hoge.fuga", children: ["ticket.hoge.fuga.finish"] },
    { key: "ticket.hoge.fuga.finish", children: [] },
    { key: "ticket.hoge.unlinked", children: [] },
  ];
  assertEquals(
    namespace.declaration.permissions.map(({ key, children }) => ({
      key,
      children,
    })),
    expectedEdges,
  );
  const expectedChildren: PermissionReference[] = [{
    kind: "namespace",
    value: { namespace: "ticket", keys: ["ticket.hoge"] },
  }];
  assertEquals(namespace.declaration.sets, [{
    set: "ticket",
    children: expectedChildren,
  }]);
  assertEquals(JSON.parse(JSON.stringify(namespace)), namespace);
});

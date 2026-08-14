import { decodalLanguage } from "decodal-codemirror";

Deno.test("editor grammar accepts Decodal 0.4 schema syntax", () => {
  const source = `
import "main.dcdl" as WorkspaceConfigSchema
{
  features = {...{ enabled = Bool; }};
  web = { enabled = Bool; ...Unknown };
}
`;
  const tree = decodalLanguage.parser.parse(source);
  const errors: string[] = [];
  tree.iterate({
    enter(node) {
      if (node.type.isError) {
        errors.push(`${node.from}..${node.to}`);
      }
    },
  });
  if (errors.length > 0) {
    throw new Error(`Decodal 0.4 grammar produced parse errors at ${errors.join(", ")}`);
  }
});

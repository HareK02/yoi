import {
  assert,
  assertFalse,
  assertMatch,
  assertStringIncludes,
} from "jsr:@std/assert";

const objectivesListSource = await Deno.readTextFile(
  new URL(
    "../src/routes/w/[workspaceId]/objectives/+page.svelte",
    import.meta.url,
  ),
);
const objectiveDetailSource = await Deno.readTextFile(
  new URL(
    "../src/routes/w/[workspaceId]/objectives/[objectiveId]/+page.svelte",
    import.meta.url,
  ),
);
const objectiveDetailLoaderSource = await Deno.readTextFile(
  new URL(
    "../src/routes/w/[workspaceId]/objectives/[objectiveId]/+page.ts",
    import.meta.url,
  ),
);
const workspaceStyles = await Deno.readTextFile(
  new URL("../src/lib/workspace/styles/workspace-pages.css", import.meta.url),
);

Deno.test("Objective list shows complete titles without body summaries", () => {
  assertStringIncludes(objectivesListSource, "{objective.title}");
  assertFalse(objectivesListSource.includes("objective.summary"));
  assertMatch(
    workspaceStyles,
    /\.objective-title\s*\{[^}]*overflow-wrap:\s*anywhere;[^}]*white-space:\s*normal;/s,
  );
});

Deno.test("Objective detail focuses on one Markdown-rendered Objective", () => {
  assertStringIncludes(
    objectiveDetailSource,
    "<RichMarkdown text={data.objective.body} />",
  );
  assertFalse(objectiveDetailSource.includes('class="objective-list'));
  assertFalse(objectiveDetailSource.includes("data.objectives"));
  assertStringIncludes(objectiveDetailSource, "linked_ticket_summaries");
  assertStringIncludes(objectiveDetailSource, "body_truncated");
});

Deno.test("Objective detail loader fetches only the selected Objective", () => {
  assertStringIncludes(
    objectiveDetailLoaderSource,
    "`/objectives/${encodeURIComponent(objectiveId)}`",
  );
  assertFalse(
    objectiveDetailLoaderSource.includes('apiPath("/objectives")'),
  );
  assertFalse(
    objectiveDetailLoaderSource.includes("ObjectiveListResponse"),
  );
  assert(
    objectiveDetailLoaderSource.includes("canonicalResourceReference"),
    "canonical Objective redirects must remain intact",
  );
});

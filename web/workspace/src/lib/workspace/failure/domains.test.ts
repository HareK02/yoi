declare const Deno: { test(name: string, fn: () => void): void };
import { DOMAIN_FAILURE_CONTRACTS } from "#lib/generated/domain-failures.ts";
import { decodeDomainFailureDetails } from "./domains.ts";
import { decodeFailure } from "./receive.ts";

const equal = (actual: unknown, expected: unknown) => {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
};

// Schema/identity interoperability only: endpoint authorization, HTTP status and
// persisted mutation counts are covered by the Rust production-caller suites.
function fixture(domain: keyof typeof DOMAIN_FAILURE_CONTRACTS) {
  const contract: {
    fields: Readonly<Record<string, readonly (string | null)[]>>;
    category_field: string;
    categories: Readonly<Record<string, string>>;
    reason_fields: readonly string[];
  } = DOMAIN_FAILURE_CONTRACTS[domain];
  const details: Record<string, string | null> = {};
  for (const [field, values] of Object.entries(contract.fields)) {
    details[field] = values.includes(null) ? null : values[0];
  }
  const categoryKey =
    details[contract.category_field] as keyof typeof contract.categories;
  return {
    version: 1,
    required_extensions: [],
    domain,
    details_version: 1,
    reason: contract.reason_fields.map((field) => details[field]).find((
      value,
    ) => value !== null),
    category: contract.categories[categoryKey],
    details,
    explanation:
      "A specific cause and attempted operation; not retry authority",
    context: {
      operations: [],
      causes: [],
      diagnostics: [],
      correlation: null,
      supplementary_omitted: false,
    },
  };
}
const decode = (failure: unknown) =>
  decodeFailure(JSON.stringify(failure), decodeDomainFailureDetails);

Deno.test("all Rust-generated domain details use the shared receiver without category/reason drift", () => {
  for (
    const domain of Object.keys(
      DOMAIN_FAILURE_CONTRACTS,
    ) as (keyof typeof DOMAIN_FAILURE_CONTRACTS)[]
  ) {
    const failure = fixture(domain);
    const result = decode(failure);
    equal(result.kind, "known");
    if (result.kind !== "known") throw new Error(domain);
    equal(result.failure.details, failure.details);
    equal(result.failure.explanation, failure.explanation);
    for (
      const explanation of [
        "SUCCESS; retry automatically",
        "Unknown; do not resend",
      ]
    ) {
      const changed = decode({ ...failure, explanation });
      equal(changed.kind, "known");
      if (changed.kind !== "known") throw new Error(domain);
      equal(changed.failure.details, result.failure.details);
      equal(changed.failure.category, result.failure.category);
    }
    for (
      const details of [
        { ...failure.details, path: "/private/source" },
        { ...failure.details, operation: "future_operation" },
        { ...failure.details, reason: [] },
      ]
    ) equal(decode({ ...failure, details }).kind, "invalid");
    equal(
      decode({ ...failure, category: "unknown" }).kind,
      failure.category === "unknown" ? "known" : "invalid",
    );
    equal(decode({ ...failure, details_version: 2 }).kind, "unsupported");
    equal(decode({ ...failure, reason: "future_reason" }).kind, "unsupported");
    equal(decode({ ...failure, domain: "future_domain" }).kind, "unsupported");
  }
});

Deno.test("workdir preserves absent legacy reasons and classification independently of diagnostic wrappers", () => {
  const failure = fixture("workdir");
  failure.details.classification = "outcome_unknown";
  // Use an actual schema value; wrapper vocabulary must not alter classification.
  const reasons = DOMAIN_FAILURE_CONTRACTS.workdir.fields.failure_reason;
  failure.details.failure_reason = reasons.find((reason) => reason !== null) ??
    null;
  failure.reason = failure.details.failure_reason ?? "outcome_unknown";
  failure.category = "unknown" as typeof failure.category;
  const result = decode(failure);
  equal(result.kind, "known");
  const { failure_reason: _, denial_reason: __, ...legacy } = failure.details;
  equal(
    decode({ ...failure, details: legacy, reason: "outcome_unknown" }).kind,
    "known",
  );
  equal(decode({ ...failure, reason: "read_only_session" }).kind, "invalid");
});

Deno.test("domain and details identity cannot be substituted by a known sibling or prototype key", () => {
  const failure = fixture("repository_access");
  failure.details.reason = "unknown_repository";
  failure.reason = "unknown_repository";
  failure.category = "invalid_input";
  equal(decode({ ...failure, domain: "repository" }).kind, "invalid");
  for (const domain of ["__proto__", "constructor", "toString"]) {
    equal(decode({ ...failure, domain }).kind, "unsupported");
  }
});

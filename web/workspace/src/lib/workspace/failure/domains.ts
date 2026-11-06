import type { FailureEnvelope } from "#lib/generated/error-contract.ts";
import {
  DOMAIN_FAILURE_CONTRACTS,
  type DomainFailureDetails,
} from "#lib/generated/domain-failures.ts";
import type { DetailsDecoder } from "./receive.ts";

// Rust domain schema/identityの生成allowlistだけを検査する。共通envelope、
// byte/depth/version/HTTPの検査はT813 receiverが所有する。category/reasonや
// 説明をHTTP分類・元操作結果・復旧/再送の判断へ変換しない。
export type DomainFieldContract =
  | readonly (string | null)[]
  | {
    readonly fields: Readonly<Record<string, DomainFieldContract>>;
    readonly required: readonly string[];
  }
  | { readonly any_of: readonly DomainFieldContract[] };
type DomainContract = {
  version: number;
  fields: Readonly<Record<string, DomainFieldContract>>;
  required: readonly string[];
  category_field: string;
  categories: Readonly<Record<string, string>>;
  reason_fields: readonly string[];
  reasons: readonly string[];
};
const contracts: Readonly<Record<string, DomainContract>> =
  DOMAIN_FAILURE_CONTRACTS;
const owns = (object: object, key: PropertyKey) =>
  Object.prototype.hasOwnProperty.call(object, key);

function isChoice(
  contract: DomainFieldContract,
): contract is readonly (string | null)[] {
  return Array.isArray(contract);
}
function accepts(contract: DomainFieldContract, value: unknown): boolean {
  if (isChoice(contract)) {
    return (typeof value === "string" || value === null) &&
      contract.includes(value);
  }
  if ("any_of" in contract) {
    return contract.any_of.some((choice) => accepts(choice, value));
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const raw = value as Record<string, unknown>;
  return Object.keys(raw).every((field) => owns(contract.fields, field)) &&
    contract.required.every((field) => owns(raw, field)) &&
    Object.entries(contract.fields).every(([field, allowed]) =>
      accepts(allowed, owns(raw, field) ? raw[field] : null)
    );
}

export const decodeDomainFailureDetails: DetailsDecoder<DomainFailureDetails> =
  (
    envelope: FailureEnvelope<unknown>,
  ) => {
    if (!owns(contracts, envelope.domain)) return { kind: "unsupported" };
    const contract = contracts[envelope.domain];
    if (
      envelope.details_version !== contract.version ||
      !contract.reasons.includes(envelope.reason)
    ) return { kind: "unsupported" };
    const details = envelope.details;
    if (
      typeof details !== "object" || details === null || Array.isArray(details)
    ) {
      return { kind: "invalid" };
    }
    const raw = details as Record<string, unknown>;
    if (
      Object.keys(raw).some((field) => !owns(contract.fields, field)) ||
      contract.required.some((field) => !owns(raw, field))
    ) return { kind: "invalid" };
    for (const [field, allowed] of Object.entries(contract.fields)) {
      // Serde Option fields may be absent. Preserve absence rather than inventing
      // a legacy reason, while using null only for the same identity comparison.
      const value = owns(raw, field) ? raw[field] : null;
      if (!accepts(allowed, value)) return { kind: "invalid" };
    }
    const reason = contract.reason_fields.map((field) => raw[field])
      .find((value) => value !== null && value !== undefined);
    const categoryKey = raw[contract.category_field];
    if (
      reason !== envelope.reason || typeof categoryKey !== "string" ||
      !owns(contract.categories, categoryKey) ||
      contract.categories[categoryKey] !== envelope.category
    ) return { kind: "invalid" };
    return { kind: "known", details: details as DomainFailureDetails };
  };

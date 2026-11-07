import { decodeDomainFailureDetails } from "#lib/workspace/failure/domains.ts";
import { failureNotice } from "#lib/workspace/failure/model.ts";
import {
  type FailureReceipt,
  receiveProblem,
} from "#lib/workspace/failure/receive.ts";
import type { DomainFailureDetails } from "#lib/generated/domain-failures.ts";
import { repositoryAccessRequestError } from "#lib/workspace/repository-access/model.ts";

export class RepositoryAccessHttpError extends Error {
  constructor(readonly receipt: FailureReceipt<DomainFailureDetails>) {
    super(failureNotice(receipt).explanation);
    this.name = "RepositoryAccessHttpError";
  }
}

// Legacy preflight/auth responses have no typed cause. Never infer one from their
// prose. Problem responses use the shared bounded receiver and public explanation.
export async function repositoryAccessResponseError(
  response: Response,
  operation: string,
): Promise<Error> {
  if (
    response.headers.get("content-type")?.split(";")[0].trim().toLowerCase() !==
      "application/problem+json"
  ) {
    await response.body?.cancel();
    return repositoryAccessRequestError(response.status, operation);
  }
  const receipt = await receiveProblem(response, decodeDomainFailureDetails);
  if (
    receipt.kind === "problem" &&
    receipt.problem.yoi_failure.domain !== "repository_access"
  ) return new RepositoryAccessHttpError({ kind: "unsupported" });
  return new RepositoryAccessHttpError(receipt);
}

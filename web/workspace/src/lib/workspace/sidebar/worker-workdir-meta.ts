import type { WorkingDirectorySummary } from "$lib/generated/workdir-api";
import type { SubscriptionWorkerWorkdirAttachment } from "$lib/generated/protocol";

const SHORT_REFERENCE_LENGTH = 12;

export type SidebarWorkdirAttachment = SubscriptionWorkerWorkdirAttachment & {
  working_directory?: WorkingDirectorySummary;
};

export type SidebarWorkdirMeta = {
  text: string;
  details: string;
};

export function sidebarWorkdirMeta(
  attachments: readonly SidebarWorkdirAttachment[],
): SidebarWorkdirMeta {
  if (attachments.length === 0) {
    return { text: "—", details: "No Workdir attachments" };
  }

  return {
    text: attachments.map(attachmentLabel).join(" · "),
    details: attachments.map(attachmentDetails).join("\n"),
  };
}

function attachmentLabel(attachment: SidebarWorkdirAttachment): string {
  const workdir = attachment.working_directory;
  const repository = clean(workdir?.repository_key) ??
    clean(attachment.repository_key) ?? "unknown-repo";
  return `${repository}:${revisionLabel(attachment)}`;
}

function revisionLabel(attachment: SidebarWorkdirAttachment): string {
  const workdir = attachment.working_directory;
  if (!workdir) {
    return `unknown@${shortReference(attachment.working_directory_id)}`;
  }

  const currentSelector = clean(workdir.current_selector);
  const currentBranch = branchName(currentSelector);
  if (currentBranch) return currentBranch;

  const currentReference = clean(workdir.current_ref);
  if (currentSelector || currentReference) {
    return explicitReferenceFallback(
      "detached",
      currentReference ?? currentSelector,
    );
  }

  const creationSelector = clean(workdir.creation_selector);
  const creationBranch = branchName(creationSelector);
  if (creationBranch) return creationBranch;

  const creationReference = clean(workdir.creation_ref);
  if (creationSelector || creationReference) {
    return explicitReferenceFallback(
      "revision",
      creationReference ?? creationSelector,
    );
  }

  return `unknown@${shortReference(attachment.working_directory_id)}`;
}

function branchName(selector: string | null): string | null {
  if (!selector || selector === "HEAD" || isHash(selector)) return null;
  if (selector.startsWith("refs/heads/")) {
    return clean(selector.slice("refs/heads/".length));
  }
  if (selector.startsWith("refs/")) return null;
  return selector;
}

function explicitReferenceFallback(
  kind: "detached" | "revision",
  reference: string | null,
): string {
  return reference ? `${kind}@${shortReference(reference)}` : kind;
}

function attachmentDetails(attachment: SidebarWorkdirAttachment): string {
  const workdir = attachment.working_directory;
  const details = [
    `${attachment.alias} — ${attachmentLabel(attachment)}`,
    `Workdir ${attachment.working_directory_id}`,
  ];
  if (clean(workdir?.current_selector)) {
    details.push(`Current selector ${workdir?.current_selector?.trim()}`);
  }
  if (clean(workdir?.current_ref)) {
    details.push(`Current ref ${workdir?.current_ref?.trim()}`);
  }
  if (clean(workdir?.creation_selector)) {
    details.push(`Creation selector ${workdir?.creation_selector?.trim()}`);
  }
  return details.join(" · ");
}

function clean(value: string | null | undefined): string | null {
  const cleaned = value?.trim();
  return cleaned ? cleaned : null;
}

function shortReference(reference: string): string {
  return reference.length > SHORT_REFERENCE_LENGTH
    ? reference.slice(0, SHORT_REFERENCE_LENGTH)
    : reference;
}

function isHash(value: string): boolean {
  return /^[0-9a-f]{7,64}$/i.test(value);
}

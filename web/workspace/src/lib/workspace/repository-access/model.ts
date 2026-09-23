export type RepositoryAccessFieldErrors = Record<string, string>;

const RESOURCE_ID = /^[A-Za-z0-9_.-]+$/;

function required(value: string, label: string): string | null {
  return value.trim() ? null : `${label} is required.`;
}

function resourceId(value: string, label: string): string | null {
  const missing = required(value, label);
  if (missing) return missing;
  if (value.length > 128) return `${label} must be 128 characters or fewer.`;
  if (!RESOURCE_ID.test(value)) {
    return `${label} may contain only letters, numbers, period, underscore, and hyphen.`;
  }
  return null;
}

function assign(
  errors: RepositoryAccessFieldErrors,
  field: string,
  message: string | null,
): void {
  if (message) errors[field] = message;
}

export function validateGeneratedCredential(input: {
  credentialId: string;
  name: string;
}): RepositoryAccessFieldErrors {
  const errors: RepositoryAccessFieldErrors = {};
  assign(
    errors,
    "credentialId",
    resourceId(input.credentialId, "Credential id"),
  );
  assign(errors, "name", required(input.name, "Name"));
  if (input.name.length > 200) {
    errors.name = "Name must be 200 characters or fewer.";
  }
  return errors;
}

export function validateImportedCredential(input: {
  credentialId: string;
  name: string;
  privateKey: string;
}): RepositoryAccessFieldErrors {
  const errors = validateGeneratedCredential(input);
  assign(
    errors,
    "privateKey",
    required(input.privateKey, "OpenSSH private key"),
  );
  return errors;
}

export function validateCredentialRotation(
  privateKey: string,
): RepositoryAccessFieldErrors {
  const errors: RepositoryAccessFieldErrors = {};
  assign(errors, "privateKey", required(privateKey, "New private key"));
  return errors;
}

export function validateHostTrust(input: {
  hostTrustId: string;
  hostname: string;
  port: number;
  hostKey: string;
}): RepositoryAccessFieldErrors {
  const errors: RepositoryAccessFieldErrors = {};
  assign(errors, "hostTrustId", resourceId(input.hostTrustId, "Host trust id"));
  assign(errors, "hostname", required(input.hostname, "Hostname"));
  if (!Number.isInteger(input.port) || input.port < 1 || input.port > 65_535) {
    errors.port = "Port must be an integer from 1 to 65535.";
  }
  assign(errors, "hostKey", required(input.hostKey, "OpenSSH public host key"));
  return errors;
}

export function repositoryAccessRequestError(
  status: number,
  operation: string,
): Error {
  if (status === 401 || status === 403) {
    return new Error(`You do not have permission to ${operation}.`);
  }
  if (status === 409) {
    return new Error(
      `This item changed before it could be saved. Reload the page, review the current value, and try again.`,
    );
  }
  if (status === 400 || status === 422) {
    return new Error(
      `The server rejected the fields for ${operation}. Review the values and try again.`,
    );
  }
  if (status === 404) {
    return new Error(
      `The item for ${operation} no longer exists. Reload the page to see the current state.`,
    );
  }
  return new Error(
    `Unable to ${operation}. Repository Access returned status ${status}.`,
  );
}

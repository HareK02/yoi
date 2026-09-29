__GENERATED_PREAMBLE__

import type {
  EventSubscriptionSelector,
  SubscriptionEventPayload,
  SubscriptionFrame,
  SubscriptionSnapshot,
  SubscriptionWorker,
  WorkspaceSubscriptionWorkdir,
} from './protocol';

export const SUBSCRIPTION_PROTOCOL_VERSION = __PROTOCOL_VERSION__;
export const MAX_SUBSCRIPTION_FRAME_JSON_BYTES = __MAX_FRAME_BYTES__;
export const MAX_SUBSCRIPTION_STRING_BYTES = __MAX_STRING_BYTES__;
export const MAX_SUBSCRIPTION_COLLECTION_ITEMS = __MAX_COLLECTION_ITEMS__;
export const MAX_SUBSCRIPTION_VALUE_DEPTH = __MAX_VALUE_DEPTH__;
export const MAX_SUBSCRIPTION_VALUE_NODES = __MAX_VALUE_NODES__;

const MAX_CORRELATION_ID_BYTES = __MAX_CORRELATION_ID_BYTES__;
const MAX_RESOURCE_ID_BYTES = __MAX_RESOURCE_ID_BYTES__;
const MAX_WORKER_IDS_PER_SELECTOR = __MAX_WORKER_IDS_PER_SELECTOR__;
const MAX_REJECTION_MESSAGE_BYTES = __MAX_REJECTION_MESSAGE_BYTES__;
const subscriptionFrameSchema: unknown = __SUBSCRIPTION_FRAME_SCHEMA__;
export const RUST_SERIALIZED_SUBSCRIPTION_FRAME_FIXTURES: readonly unknown[] =
  __RUST_SERIALIZED_FIXTURES__;
const utf8 = new TextEncoder();

type JsonObject = Record<string, unknown>;
type RejectionReason =
  | 'invalid_transport'
  | 'frame_too_large'
  | 'invalid_json'
  | 'aggregate_limit'
  | 'invalid_structure'
  | 'invalid_semantics';

export type SubscriptionFrameDecodeResult =
  | { ok: true; value: SubscriptionFrame }
  | { ok: false; reason: RejectionReason };

/**
 * Decode an untrusted Workspace WebSocket message without reflecting its content.
 * A successful result has passed aggregate limits, the exact generated JSON schema,
 * and the Rust protocol's Browser-relevant semantic checks.
 */
export function decodeSubscriptionFrame(input: unknown): SubscriptionFrameDecodeResult {
  if (typeof input !== 'string') return { ok: false, reason: 'invalid_transport' };
  if (utf8.encode(input).byteLength > MAX_SUBSCRIPTION_FRAME_JSON_BYTES) {
    return { ok: false, reason: 'frame_too_large' };
  }

  let candidate: unknown;
  try {
    candidate = JSON.parse(input) as unknown;
  } catch {
    return { ok: false, reason: 'invalid_json' };
  }
  if (!withinAggregateLimits(candidate)) return { ok: false, reason: 'aggregate_limit' };
  if (!validateSchema(candidate, subscriptionFrameSchema)) {
    return { ok: false, reason: 'invalid_structure' };
  }

  // The assertion follows generated structural validation; it is not a parse-time cast.
  const frame = candidate as SubscriptionFrame;
  if (!validateInboundFrameSemantics(frame)) {
    return { ok: false, reason: 'invalid_semantics' };
  }
  return { ok: true, value: frame };
}

/** Validate stateful selector/payload consistency after the multiplexer resolves routing. */
export function subscriptionFrameMatchesSelector(
  frame: SubscriptionFrame,
  selector: EventSubscriptionSelector,
): boolean {
  if (frame.frame === 'response' && frame.message.result === 'subscribed') {
    return selectorsEqual(frame.message.payload.selector, selector);
  }
  if (frame.frame === 'event' && frame.message.event === 'event') {
    return eventPayloadMatchesSelector(frame.message.data.payload, selector);
  }
  return true;
}

function withinAggregateLimits(root: unknown): boolean {
  let nodes = 0;
  const visit = (value: unknown, depth: number): boolean => {
    nodes += 1;
    if (nodes > MAX_SUBSCRIPTION_VALUE_NODES || depth > MAX_SUBSCRIPTION_VALUE_DEPTH) return false;
    if (typeof value === 'string') {
      return utf8.encode(value).byteLength <= MAX_SUBSCRIPTION_STRING_BYTES;
    }
    if (typeof value === 'number') {
      return Number.isFinite(value) && (!Number.isInteger(value) || Number.isSafeInteger(value));
    }
    if (value === null || typeof value === 'boolean') return true;
    if (Array.isArray(value)) {
      return (
        value.length <= MAX_SUBSCRIPTION_COLLECTION_ITEMS &&
        value.every((entry) => visit(entry, depth + 1))
      );
    }
    if (!isObject(value)) return false;
    const entries = Object.entries(value);
    return (
      entries.length <= MAX_SUBSCRIPTION_COLLECTION_ITEMS &&
      entries.every(
        ([key, entry]) => visit(key, depth + 1) && visit(entry, depth + 1),
      )
    );
  };
  return visit(root, 0);
}

function validateSchema(value: unknown, rawSchema: unknown): boolean {
  if (rawSchema === true) return true;
  if (rawSchema === false || !isObject(rawSchema)) return false;
  const schema = rawSchema;

  if (typeof schema.$ref === 'string') {
    const target = resolveLocalReference(schema.$ref);
    if (target === undefined || !validateSchema(value, target)) return false;
  }
  if ('const' in schema && !jsonEqual(value, schema.const)) return false;
  if (Array.isArray(schema.enum) && !schema.enum.some((entry) => jsonEqual(value, entry))) {
    return false;
  }
  if (Array.isArray(schema.oneOf)) {
    let matches = 0;
    for (const alternative of schema.oneOf) {
      if (validateSchema(value, alternative)) matches += 1;
    }
    if (matches !== 1) return false;
  }
  if (Array.isArray(schema.anyOf) && !schema.anyOf.some((entry) => validateSchema(value, entry))) {
    return false;
  }

  if (schema.type !== undefined && !matchesType(value, schema.type)) return false;
  if (typeof value === 'number') {
    if (schema.type === 'integer' && !Number.isSafeInteger(value)) return false;
    if (typeof schema.minimum === 'number' && value < schema.minimum) return false;
    if (typeof schema.maximum === 'number' && value > schema.maximum) return false;
  }
  if (typeof value === 'string') {
    const length = [...value].length;
    if (typeof schema.minLength === 'number' && length < schema.minLength) return false;
    if (typeof schema.maxLength === 'number' && length > schema.maxLength) return false;
  }
  if (Array.isArray(value)) {
    if (typeof schema.minItems === 'number' && value.length < schema.minItems) return false;
    if (typeof schema.maxItems === 'number' && value.length > schema.maxItems) return false;
    if (schema.items !== undefined && !value.every((entry) => validateSchema(entry, schema.items))) {
      return false;
    }
  }
  if (isObject(value) && schema.properties !== undefined) {
    if (!isObject(schema.properties)) return false;
    const properties = schema.properties;
    if (Array.isArray(schema.required)) {
      for (const key of schema.required) {
        if (typeof key !== 'string' || !Object.prototype.hasOwnProperty.call(value, key)) return false;
      }
    }
    for (const [key, entry] of Object.entries(value)) {
      if (Object.prototype.hasOwnProperty.call(properties, key)) {
        if (!validateSchema(entry, properties[key])) return false;
      } else if (schema.additionalProperties === false) {
        return false;
      } else if (isObject(schema.additionalProperties) || typeof schema.additionalProperties === 'boolean') {
        if (!validateSchema(entry, schema.additionalProperties)) return false;
      }
    }
  }
  return true;
}

function resolveLocalReference(reference: string): unknown {
  if (!reference.startsWith('#/')) return undefined;
  let current: unknown = subscriptionFrameSchema;
  for (const encoded of reference.slice(2).split('/')) {
    if (!isObject(current)) return undefined;
    const key = encoded.replaceAll('~1', '/').replaceAll('~0', '~');
    current = current[key];
  }
  return current;
}

function matchesType(value: unknown, rawType: unknown): boolean {
  if (Array.isArray(rawType)) return rawType.some((entry) => matchesType(value, entry));
  switch (rawType) {
    case 'null':
      return value === null;
    case 'boolean':
      return typeof value === 'boolean';
    case 'string':
      return typeof value === 'string';
    case 'number':
      return typeof value === 'number' && Number.isFinite(value);
    case 'integer':
      return typeof value === 'number' && Number.isSafeInteger(value);
    case 'array':
      return Array.isArray(value);
    case 'object':
      return isObject(value);
    default:
      return false;
  }
}

function isObject(value: unknown): value is JsonObject {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function jsonEqual(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (Array.isArray(left) && Array.isArray(right)) {
    return left.length === right.length && left.every((entry, index) => jsonEqual(entry, right[index]));
  }
  if (isObject(left) && isObject(right)) {
    const leftKeys = Object.keys(left);
    const rightKeys = Object.keys(right);
    return (
      leftKeys.length === rightKeys.length &&
      leftKeys.every(
        (key) => Object.prototype.hasOwnProperty.call(right, key) && jsonEqual(left[key], right[key]),
      )
    );
  }
  return false;
}

function validateInboundFrameSemantics(frame: SubscriptionFrame): boolean {
  if (frame.protocol_version !== SUBSCRIPTION_PROTOCOL_VERSION) return false;
  if (frame.frame === 'request' || frame.frame === 'worker_protocol') return false;
  if (frame.frame === 'response') {
    const message = frame.message;
    if (!validateIdentifier(message.payload.request_id, MAX_CORRELATION_ID_BYTES)) return false;
    if (message.result === 'subscribed') {
      return (
        validateIdentifier(message.payload.subscription_id, MAX_CORRELATION_ID_BYTES) &&
        validateSelector(message.payload.selector) &&
        snapshotMatchesSelector(message.payload.snapshot, message.payload.selector)
      );
    }
    if (message.result === 'unsubscribed') {
      return validateIdentifier(message.payload.subscription_id, MAX_CORRELATION_ID_BYTES);
    }
    return (
      (message.payload.subscription_id == null ||
        validateIdentifier(message.payload.subscription_id, MAX_CORRELATION_ID_BYTES)) &&
      validateBoundedMessage(message.payload.message)
    );
  }

  const message = frame.message;
  if (!validateIdentifier(message.data.subscription_id, MAX_CORRELATION_ID_BYTES)) return false;
  if (message.event === 'subscription_closed') {
    return validateBoundedMessage(message.data.message);
  }
  return validateEventPayload(message.data.payload);
}

function validateSelector(selector: EventSubscriptionSelector): boolean {
  if (selector.topic === 'worker_lifecycle') {
    if (
      selector.worker_ids.length === 0 ||
      selector.worker_ids.length > MAX_WORKER_IDS_PER_SELECTOR
    ) {
      return false;
    }
    const ids = new Set<string>();
    for (const workerId of selector.worker_ids) {
      if (!validateIdentifier(workerId, MAX_RESOURCE_ID_BYTES) || ids.has(workerId)) return false;
      ids.add(workerId);
    }
    return true;
  }
  if (selector.topic === 'worker_protocol') {
    return (
      validateIdentifier(selector.worker_id, MAX_RESOURCE_ID_BYTES) &&
      (selector.runtime_id == null ||
        validateIdentifier(selector.runtime_id, MAX_RESOURCE_ID_BYTES))
    );
  }
  return true;
}

function snapshotMatchesSelector(
  snapshot: SubscriptionSnapshot,
  selector: EventSubscriptionSelector,
): boolean {
  if (!validateSelector(selector)) return false;
  if (selector.topic === 'runtime_workers' || selector.topic === 'workspace_workers') {
    return snapshot.topic === 'workers' && validateWorkers(snapshot.data.workers);
  }
  if (selector.topic === 'worker_lifecycle') {
    const selected = new Set(selector.worker_ids);
    return (
      snapshot.topic === 'workers' &&
      validateWorkers(snapshot.data.workers) &&
      snapshot.data.workers.every((worker) => selected.has(worker.worker_id))
    );
  }
  if (selector.topic === 'worker_protocol') {
    return snapshot.topic === 'worker_protocol' && snapshot.data.worker_id === selector.worker_id;
  }
  return (
    snapshot.topic === 'workspace_workdirs' &&
    snapshot.data.workdirs.every(validateWorkspaceWorkdir)
  );
}

function validateEventPayload(payload: SubscriptionEventPayload): boolean {
  switch (payload.event) {
    case 'worker_upserted':
      return validateWorker(payload.data.worker);
    case 'worker_removed':
      return (
        validateIdentifier(payload.data.worker_id, MAX_RESOURCE_ID_BYTES) &&
        (payload.data.runtime_id == null ||
          validateIdentifier(payload.data.runtime_id, MAX_RESOURCE_ID_BYTES))
      );
    case 'worker_protocol':
      return validateIdentifier(payload.data.worker_id, MAX_RESOURCE_ID_BYTES);
    case 'workdir_upserted':
      return validateWorkspaceWorkdir(payload.data.workdir);
    case 'workdir_removed':
      return validateIdentifier(payload.data.working_directory_id, MAX_RESOURCE_ID_BYTES);
  }
}

function eventPayloadMatchesSelector(
  payload: SubscriptionEventPayload,
  selector: EventSubscriptionSelector,
): boolean {
  if (selector.topic === 'runtime_workers' || selector.topic === 'workspace_workers') {
    return payload.event === 'worker_upserted' || payload.event === 'worker_removed';
  }
  if (selector.topic === 'worker_lifecycle') {
    return (
      (payload.event === 'worker_upserted' && selector.worker_ids.includes(payload.data.worker.worker_id)) ||
      (payload.event === 'worker_removed' && selector.worker_ids.includes(payload.data.worker_id))
    );
  }
  if (selector.topic === 'worker_protocol') {
    return payload.event === 'worker_protocol' && payload.data.worker_id === selector.worker_id;
  }
  return payload.event === 'workdir_upserted' || payload.event === 'workdir_removed';
}

function selectorsEqual(
  left: EventSubscriptionSelector,
  right: EventSubscriptionSelector,
): boolean {
  if (left.topic !== right.topic) return false;
  if (left.topic === 'worker_lifecycle' && right.topic === 'worker_lifecycle') {
    const rightIds = new Set(right.worker_ids);
    return left.worker_ids.length === rightIds.size && left.worker_ids.every((workerId) => rightIds.has(workerId));
  }
  if (left.topic === 'worker_protocol' && right.topic === 'worker_protocol') {
    return (
      left.worker_id === right.worker_id &&
      (left.runtime_id ?? null) === (right.runtime_id ?? null)
    );
  }
  return true;
}

function validateWorkers(workers: SubscriptionWorker[]): boolean {
  const identities = new Set<string>();
  for (const worker of workers) {
    const identity = `${worker.runtime_id ?? ''}\u0000${worker.worker_id}`;
    if (!validateWorker(worker) || identities.has(identity)) return false;
    identities.add(identity);
  }
  return true;
}

function validateWorker(worker: SubscriptionWorker): boolean {
  if (!validateIdentifier(worker.worker_id, MAX_RESOURCE_ID_BYTES)) return false;
  if (worker.runtime_id != null && !validateIdentifier(worker.runtime_id, MAX_RESOURCE_ID_BYTES)) {
    return false;
  }
  if (worker.resource_key != null && !validateIdentifier(worker.resource_key, MAX_RESOURCE_ID_BYTES)) {
    return false;
  }
  const aliases = new Set<string>();
  const workdirIds = new Set<string>();
  for (const attachment of worker.workdir_attachments ?? []) {
    if (
      !validateIdentifier(attachment.alias, MAX_RESOURCE_ID_BYTES) ||
      !validateIdentifier(attachment.working_directory_id, MAX_RESOURCE_ID_BYTES) ||
      (attachment.repository_key != null && !validateRepositoryKey(attachment.repository_key)) ||
      aliases.has(attachment.alias) ||
      workdirIds.has(attachment.working_directory_id)
    ) {
      return false;
    }
    aliases.add(attachment.alias);
    workdirIds.add(attachment.working_directory_id);
  }
  return true;
}

function validateWorkspaceWorkdir(workdir: WorkspaceSubscriptionWorkdir): boolean {
  return (
    validateIdentifier(workdir.working_directory_id, MAX_RESOURCE_ID_BYTES) &&
    validateRepositoryKey(workdir.repository_key) &&
    validateIdentifier(workdir.state, MAX_RESOURCE_ID_BYTES)
  );
}

function validateIdentifier(value: string, maxBytes: number): boolean {
  return (
    value.length > 0 &&
    utf8.encode(value).byteLength <= maxBytes &&
    value.trim() === value &&
    !/\p{Cc}/u.test(value)
  );
}

function validateRepositoryKey(value: string): boolean {
  return (
    utf8.encode(value).byteLength <= 64 &&
    /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(value)
  );
}

function validateBoundedMessage(value: string): boolean {
  const bytes = utf8.encode(value).byteLength;
  return bytes > 0 && bytes <= MAX_REJECTION_MESSAGE_BYTES;
}

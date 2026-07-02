import {
  buildBrowserCreateWorkerRequest,
  defaultWorkerLaunchForm,
  type WorkerLaunchFormState,
} from './worker-launch.ts';

import type { WorkerLaunchOptionsResponse } from './types.ts';

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

const options: WorkerLaunchOptionsResponse = {
  workspace_id: 'workspace',
  runtimes: [
    {
      runtime_id: 'remote-runtime',
      display_name: 'Remote Runtime',
      built_in: false,
      can_spawn_worker: false,
      status: 'active',
      diagnostics: [],
    },
    {
      runtime_id: 'embedded-worker-runtime',
      display_name: 'Embedded Runtime',
      built_in: true,
      can_spawn_worker: true,
      status: 'active',
      diagnostics: [],
    },
  ],
  profiles: [
    {
      id: 'runtime_default',
      label: 'Runtime default',
      description: 'Runtime default profile.',
    },
    {
      id: 'builtin:coder',
      label: 'Coding Worker',
      description: 'Coding role.',
    },
  ],
  diagnostics: [],
};

Deno.test('new worker form defaults to backend-published runtime and profile candidates', () => {
  const current: WorkerLaunchFormState = {
    runtime_id: '',
    display_name: '',
    profile: 'free-text-profile',
    initial_text: 'start here',
  };

  const form = defaultWorkerLaunchForm(options, current);
  assert(form.runtime_id === 'embedded-worker-runtime', 'should choose spawn-capable runtime');
  assert(form.profile === 'builtin:coder', 'should choose backend-published coder profile');
  assert(form.display_name === 'Coding Worker', 'should derive default display name');
  assert(form.initial_text === 'start here', 'should preserve initial text');
});

Deno.test('new worker submit payload exposes only browser contract fields', () => {
  const request = buildBrowserCreateWorkerRequest({
    runtime_id: 'embedded-worker-runtime',
    display_name: 'Coding Worker',
    profile: 'builtin:coder',
    initial_text: 'implement ticket',
  });

  assert(
    JSON.stringify(Object.keys(request).sort()) ===
      JSON.stringify(['display_name', 'initial_text', 'profile', 'runtime_id'].sort()),
    'submit payload should contain only Browser-facing worker create fields',
  );
  assert(!('kind' in request), 'kind must not be exposed as a Browser request field');
});

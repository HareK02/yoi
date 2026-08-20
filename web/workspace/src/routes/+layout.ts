import type { LayoutLoad } from './$types';

// Workspace selection is explicit at `/`; the root layout must never infer a
// singleton Workspace or redirect based on an unscoped compatibility endpoint.
export const load: LayoutLoad = () => ({});

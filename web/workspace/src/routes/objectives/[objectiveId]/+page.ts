import type { PageLoad } from './$types';

export const load: PageLoad = ({ params }) => ({
  objectiveId: params.objectiveId
});

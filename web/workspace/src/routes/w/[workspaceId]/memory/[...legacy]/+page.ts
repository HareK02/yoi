import { error } from "@sveltejs/kit";
import type { PageLoad } from "./$types";

export const load: PageLoad = ({ params }) => {
  // Subject名stagingの配下URLは移行対象だが、旧Workspace候補一覧は廃止済み。
  if (params.legacy === "staging" || params.legacy === "staging/") {
    error(404, "Not Found");
  }
};

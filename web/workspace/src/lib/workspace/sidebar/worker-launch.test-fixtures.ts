import type { WorkerLaunchOptionsResponse } from "./types.ts";

export function emptyLaunchOptions(
  workspaceId: string,
): WorkerLaunchOptionsResponse {
  return {
    workspace_id: workspaceId,
    runtimes: [
      {
        runtime_id: "embedded",
        display_name: "Embedded",
        status: "active",
        worker_creation_available: true,
        built_in: true,
        working_directory_required: false,
        diagnostics: [],
      },
      {
        runtime_id: "remote",
        display_name: "Remote",
        status: "active",
        worker_creation_available: true,
        built_in: false,
        working_directory_required: true,
        diagnostics: [],
      },
    ],
    default_profile: "builtin:companion",
    profiles: [{
      id: "builtin:companion",
      label: "Companion",
      description: "Conversation",
      feature_connections: { subjektiv: false },
    }],
    repositories: [],
    working_directories: [],
    diagnostics: [],
  };
}

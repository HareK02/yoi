# Decision: remove generic event channels and standardize BackgroundTask

The Plugin/Feature base Pod API should not expose a plugin-defined event-channel mechanism for arbitrary structured client/UI payloads.

Revised boundary:

- Model-visible notifications use the existing durable `Method::Notify` / `SystemItem::Notification` / `Event::SystemItem` path. If the model can see it, it must be committed to history and visible to users on replay/inspection.
- `Event::Alert`-like output is a short transient human-facing alert only. It is not model-visible, not session history, and not a structured UI extension channel.
- Diagnostics/status are host-defined operational records for install/runtime/capability/task reporting, not arbitrary plugin UI messages.
- Dialogs/confirmations/custom UI are deferred. If needed later, they should be a separate host-defined interaction protocol, not a generic plugin event channel.
- `BackgroundTaskContribution` is a first-class contribution kind. The host starts, tracks, cancels, and reports background tasks; feature modules must not spawn untracked async loops. Task output is limited to granted sinks/services: model notification, alert, diagnostics, and host-granted services.

This keeps the Plugin API centered on Tools, safe Hooks, host-managed BackgroundTasks, durable model-visible notifications, and bounded host-defined operational reporting.

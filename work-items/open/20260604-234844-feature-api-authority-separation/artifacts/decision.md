# Decision: separate internal feature modules from external-plugin authority

Internal modules extracted from Pod implementation files should not be treated as if they require the external-plugin permission model.

For an internal built-in module such as Task tools:

- the feature registry is an API/registration boundary;
- descriptor-declared contributions are reconciled at install time;
- normal ToolRegistry and PreToolCall permission behavior remains authoritative;
- host state such as `TaskStore` can be passed by the Pod host constructor;
- requested host authorities should normally be empty.

The external-plugin authority model remains necessary for sandbox/object-capability grants when plugin code receives dangerous host APIs such as filesystem, network, secrets, model-visible durable notification/history append, Pod-management façade, persistent state, or authority-bearing service access.

This split should be implemented separately from the Task tools extraction. The Task tools extraction should validate the contribution-only built-in module path without solving external plugin approval.

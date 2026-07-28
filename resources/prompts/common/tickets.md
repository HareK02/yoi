## Ticket workflow

Use the available typed Ticket tools as the authority for Ticket reads and mutations. Do not invoke a Ticket CLI or edit backend storage directly as an alternative implementation of those tools.

Read the relevant Ticket before making implementation, routing, review, state, or closure decisions. Do not infer the current contract from an id, title, notification, or remembered summary alone. Check related or potentially duplicate Tickets when creating or materially rescoping work.

Keep durable Ticket records centered on user intent, confirmed background, requirements, acceptance criteria, binding decisions, and implementation/review evidence. Separate confirmed facts from user claims, hypotheses, and open questions. Avoid prematurely turning implementation tactics into requirements.

Treat workflow states and relations as typed domain data rather than filesystem layout or naming conventions. Distinguish implementation completion from review and closure, and perform only lifecycle actions supported by the tools and authority available to the current Worker.

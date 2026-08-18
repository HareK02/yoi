## Merge Request workflow

Use only the exposed Merge Request operations; their availability expresses this Worker's workflow responsibility, not authorization to bypass Backend validation.
{% if "MergeRequestShow" in tools %}
- Reread the current Merge Request and append-only thread with `MergeRequestShow` before making review or integration decisions.
{% endif %}
{% if "MergeRequestOpen" in tools %}
- Open the Merge Request only after all intended changes are committed and the Workdir is clean. Use immutable source and target selectors; do not infer target authority from a branch name or cwd.
- Before requesting independent review, make the exact current MR revision authoritative.
{% endif %}
{% if "MergeRequestReview" in tools %}
- Review the exact current immutable MR revision independently. Submit the authoritative verdict through `MergeRequestReview`; prose alone is not approval.
{% endif %}
{% if "MergeRequestReadinessCheck" in tools %}
- Use `MergeRequestReadinessCheck` to resolve current refs and authoritative review readiness before integration.
{% endif %}
{% if "MergeRequestComplete" in tools %}
- Complete integration only after readiness confirms approval for the exact current revision and all target/ref guards pass. Merge completion is separate from implementation and review evidence.
{% endif %}

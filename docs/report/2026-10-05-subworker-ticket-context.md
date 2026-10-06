# Delegated coding context and artifact access

During T-696 recovery, the parent Coder could read the authoritative Ticket through `ShowTicket`, but default coding SubWorkers did not receive that tool. A delegated task that simply asks a child to reread the Ticket therefore cannot be fulfilled. One child attempted a CLI fallback; this is not an authorized substitute for typed Backend tools. The parent supplied the authoritative revision and relevant requirements instead and retained lifecycle/publish authority.

Improvement: make the child tool/capability projection explicit in the spawn result, and support passing a bounded authoritative Ticket snapshot with revision into a child without implying mutation authority. Delegation prompts should include the necessary contract rather than assuming the child has the parent's Backend tools.

The Bash provider also advertised a full-output spill under the parent's declared readable `bash-output` directory, but `Read` rejected the returned absolute path as outside the attachment scope. The parent did not bypass that rejection; it used smaller bounded queries. Improvement: return a typed read capability/artifact reference for spill output, or ensure the advertised spill path is actually readable through the selected attachment.

A child spawned with `cwd: tools/web-ux` observed its filesystem attachment rooted there, and sibling `docs`, `web/workspace`, and `target/web-ux` reads were rejected despite explicit sibling scope rules. The tool description says `cwd` changes only the default command directory. The parent stopped the unchanged child and created a root-oriented child with an explicit nonrecursive readable root plus the original bounded sibling rules. Improvement: preserve attachment-relative path identity independently of command cwd, or document the actual child path binding in the spawn result. Capability rejections should remain authoritative, not trigger filesystem/CLI fallbacks.


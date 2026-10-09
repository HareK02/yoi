# T-724: omitted SubWorker cwd can omit the checkout attachment

During T-724, children created with read-only `.` and restricted writable logical paths but no explicit `cwd` had inconsistent startup contexts. The API child could read the checkout; the browser and real-roundtrip children only saw their transient bash-output directory and reported no repository Workdir/command attachment. Neither blocked child edited source or ran validation. Their committed sessions were inspected before stopping them.

Recreating those two children with **the same readable/writable rules** and explicit `cwd: "."` at the checkout root (using the inherited Coder profile) restored repository tools. There was no denied-write bypass, broadened filesystem scope, absolute-path fallback or Backend storage mutation. The recreated children successfully ran production-shell visual captures and a real isolated SQLite/LocalFileSystem/Server/Web-adapter test respectively.

This repeats the omitted-cwd case described in `t718-subworker-path-context.md`. The spawn response should expose actual attachment aliases/root/tool cwd, and reject or clearly report a scope whose attachment could not be bound. The filesystem logical root should remain invariant; cwd is documented as a command default, not an authority grant. This report records tooling friction, not a new Drive requirement or an execution prohibition.

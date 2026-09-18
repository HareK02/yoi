<script lang="ts">
  import Bevel from '$lib/workspace/ui/Bevel.svelte';
  import BevelLine from '$lib/workspace/ui/BevelLine.svelte';
  import Tooltip from '$lib/workspace/ui/Tooltip.svelte';
</script>

<div class="showroom">
  <section class="showroom-section" aria-labelledby="action-heading">
    <h1 id="action-heading" class="showroom-heading">Action</h1>
    <BevelLine direction="x" length="100%" decorative />
    <div class="action-row">
      <Bevel>
        <button class="action action--primary" type="button">Create Worker</button>
      </Bevel>
      <Tooltip id="rerun-help" text="Runs validation against the current Workspace configuration." placement="bottom" align="start">
        {#snippet children(descriptionId)}
          <Bevel>
            <button class="action action--secondary" type="button" aria-describedby={descriptionId}>Re-run checks</button>
          </Bevel>
        {/snippet}
      </Tooltip>
      <button class="action action--text" type="button">View log</button>
      <Bevel>
        <button class="action action--destructive" type="button">Remove repository</button>
      </Bevel>
      <Bevel>
        <button class="action action--secondary" type="button" disabled>Running checks…</button>
      </Bevel>
    </div>

    <div class="border-matrix">
      <Bevel as="div" fill>
        <div class="tactile-group" role="group" aria-label="Bordered validation controls">
          <span>
            <strong>Closed border</strong>
            <small>3 checks passed at 14:32</small>
          </span>
          <Bevel>
            <button class="compact-control" type="button" aria-pressed="true">Live</button>
          </Bevel>
        </div>
      </Bevel>
      <Bevel as="div" fill>
        <div class="tactile-group" role="group" aria-label="Read-only bordered controls">
          <span>
            <strong>Read-only border</strong>
            <small>Read-only control group</small>
          </span>
          <span class="status"><span class="status__marker" data-tone="muted"></span>Locked</span>
        </div>
      </Bevel>
      <Bevel as="div" right={false} bottom={false} fill>
        <div class="tactile-group" role="group" aria-label="Top and left edge">
          <span>
            <strong>Top + left</strong>
            <small>Only the shared top-left corner is rounded</small>
          </span>
        </div>
      </Bevel>
      <Bevel as="div" top={false} left={false} fill>
        <div class="tactile-group" role="group" aria-label="Right and bottom groove">
          <span>
            <strong>Right + bottom</strong>
            <small>Only the shared bottom-right corner is rounded</small>
          </span>
        </div>
      </Bevel>
    </div>
  </section>

  <section class="showroom-section" aria-labelledby="line-heading">
    <h2 id="line-heading" class="showroom-heading">Line</h2>
    <BevelLine direction="x" length="100%" decorative />
    <div class="line-showcase">
      <div class="line-sample line-sample--horizontal">
        <span class="line-sample__label">Horizontal · 100%</span>
        <BevelLine direction="x" length="100%" />
        <span class="line-sample__label">Horizontal · 160px</span>
        <BevelLine direction="x" length="160px" />
      </div>
      <div class="line-sample line-sample--vertical">
        <span>Before</span>
        <BevelLine direction="y" length="72px" />
        <span>After</span>
      </div>
    </div>
  </section>

  <section class="showroom-section" aria-labelledby="status-heading">
    <h2 id="status-heading" class="showroom-heading">Status</h2>
    <BevelLine direction="x" length="100%" decorative />
    <div class="status-row">
      <span class="status"><span class="status__marker" data-tone="success"></span>Ready</span>
      <span class="status"><span class="status__marker" data-tone="accent"></span>Running</span>
      <span class="status"><span class="status__marker" data-tone="warning"></span>Needs review</span>
      <span class="status"><span class="status__marker" data-tone="danger"></span>Failed</span>
      <span class="status"><span class="status__marker" data-tone="muted"></span>Unavailable</span>
    </div>
  </section>

  <section class="showroom-section" aria-labelledby="feedback-heading">
    <h2 id="feedback-heading" class="showroom-heading">Feedback</h2>
    <BevelLine direction="x" length="100%" decorative />
    <div class="feedback-grid">
      <div class="feedback" role="status">
        <span class="status"><span class="status__marker status__marker--loading" data-tone="accent"></span>Loading Merge Requests…</span>
      </div>
      <div class="feedback">
        <strong>No Objectives.</strong>
      </div>
      <div class="feedback feedback--error" role="alert">
        <div>
          <strong>Merge failed.</strong>
          <p>Target branch changed after review. Refresh the target before retrying.</p>
        </div>
        <Bevel>
          <button class="action action--secondary" type="button">Refresh target</button>
        </Bevel>
      </div>
      <div class="feedback feedback--permission">
        <div>
          <strong>Repository Access is read-only.</strong>
          <p>Workspace owner permission is required to change credentials.</p>
        </div>
      </div>
    </div>
  </section>

  <section id="resource-row" class="showroom-section" aria-labelledby="resource-row-heading">
    <h2 id="resource-row-heading" class="showroom-heading">Resource row</h2>
    <BevelLine direction="x" length="100%" decorative />
    <div class="resource-list">
      <a class="resource-row" href="/design-lab/workspace-web-ux?ticket=YOI-248">
        <span class="resource-row__identity">
          <code>YOI-248</code>
          <strong>Preserve nested navigation ownership across route transitions</strong>
        </span>
        <span class="resource-row__meta">High priority</span>
        <span class="status"><span class="status__marker" data-tone="warning"></span>Blocked</span>
      </a>
      <BevelLine direction="x" length="100%" decorative />
      <a class="resource-row" href="/design-lab/workspace-web-ux?ticket=YOI-251">
        <span class="resource-row__identity">
          <code>YOI-251</code>
          <strong>Validate repository access projection</strong>
        </span>
        <span class="resource-row__meta">Updated 14 min ago</span>
        <span class="status"><span class="status__marker" data-tone="success"></span>Ready</span>
      </a>
      <BevelLine direction="x" length="100%" decorative />
      <a class="resource-row" href="/design-lab/workspace-web-ux?ticket=YOI-252">
        <span class="resource-row__identity">
          <code>YOI-252-WITH-A-LONG-CANONICAL-RESOURCE-KEY</code>
          <strong>Keep very long translated resource names readable without pushing the current state out of the row</strong>
        </span>
        <span class="resource-row__meta">No owner</span>
        <span class="status"><span class="status__marker" data-tone="muted"></span>Draft</span>
      </a>
    </div>
  </section>

  <section class="showroom-section" aria-labelledby="comparable-heading">
    <h2 id="comparable-heading" class="showroom-heading">Comparable data</h2>
    <BevelLine direction="x" length="100%" decorative />
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (Scrollable comparison must be keyboard-focusable.) -->
    <div class="table-scroll" tabindex="0" role="region" aria-label="Runtime comparison">
      <table>
        <thead>
          <tr>
            <th scope="col">Runtime</th>
            <th scope="col">Kind</th>
            <th scope="col">State</th>
            <th scope="col">
              <Tooltip id="last-seen-help" text="Most recent observation from the Runtime host." placement="top" align="end">
                {#snippet children(descriptionId)}
                  <!-- svelte-ignore a11y_no_noninteractive_tabindex (Focusable column label exposes contextual help to keyboard users.) -->
                  <span class="column-help" tabindex="0" aria-describedby={descriptionId}>Last seen</span>
                {/snippet}
              </Tooltip>
            </th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <th scope="row"><span>Local development</span><code>local</code></th>
            <td>Built-in</td>
            <td><span class="status"><span class="status__marker" data-tone="success"></span>Available</span></td>
            <td>Now</td>
          </tr>
          <tr>
            <th scope="row"><span>Remote Linux runner with a long operational name</span><code>runner-eu-central-01</code></th>
            <td>Remote</td>
            <td><span class="status"><span class="status__marker" data-tone="danger"></span>Unavailable</span></td>
            <td>2 hours ago</td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>

  <div class="showroom-split">
    <section class="showroom-section" aria-labelledby="key-value-heading">
      <h2 id="key-value-heading" class="showroom-heading">Key-value</h2>
      <BevelLine direction="x" length="100%" decorative />
      <dl class="key-value">
        <div><dt>Repository</dt><dd>yoi</dd></div>
        <BevelLine as="div" direction="x" length="100%" decorative />
        <div><dt>Default selector</dt><dd><code>refs/heads/main</code></dd></div>
        <BevelLine as="div" direction="x" length="100%" decorative />
        <div><dt>Access</dt><dd><span class="status"><span class="status__marker" data-tone="success"></span>Configured</span></dd></div>
      </dl>
    </section>

    <section class="showroom-section" aria-labelledby="tooltip-heading">
      <h2 id="tooltip-heading" class="showroom-heading">Tooltip</h2>
      <BevelLine direction="x" length="100%" decorative />
      <Tooltip id="source-help" text="The source that currently supplies this configuration value." placement="bottom" align="start">
        {#snippet children(descriptionId)}
          <button class="help-label" type="button" aria-describedby={descriptionId}>Configuration source <span aria-hidden="true">?</span></button>
        {/snippet}
      </Tooltip>
    </section>
  </div>

  <section class="showroom-section" aria-labelledby="disclosure-heading">
    <h2 id="disclosure-heading" class="showroom-heading">Disclosure</h2>
    <BevelLine direction="x" length="100%" decorative />
    <details class="disclosure">
      <summary>Technical details</summary>
      <dl class="technical-values">
        <div><dt>Revision</dt><dd><code>rev_01J8WZ7C8AV6P3Y2N0M4KQ9B1F</code></dd></div>
        <BevelLine as="div" direction="x" length="100%" decorative />
        <div><dt>Digest</dt><dd><code>sha256:2f8d9bb168afb7b19f447463f3fbe8b4728d8e87a64eaf1afbea1ca4d4c41f21</code></dd></div>
      </dl>
    </details>
  </section>

  <section class="showroom-section" aria-labelledby="color-heading">
    <h2 id="color-heading" class="showroom-heading">Color</h2>
    <BevelLine direction="x" length="100%" decorative />
    <div class="token-grid">
      <div class="color-token"><span style="--sample: var(--bg)"></span><code>--bg</code></div>
      <div class="color-token"><span style="--sample: var(--bg-raised)"></span><code>--bg-raised</code></div>
      <div class="color-token"><span style="--sample: var(--bg-subtle)"></span><code>--bg-subtle</code></div>
      <div class="color-token"><span style="--sample: var(--text-strong)"></span><code>--text-strong</code></div>
      <div class="color-token"><span style="--sample: var(--accent)"></span><code>--accent</code></div>
      <div class="color-token"><span style="--sample: var(--success)"></span><code>--success</code></div>
      <div class="color-token"><span style="--sample: var(--warning)"></span><code>--warning</code></div>
      <div class="color-token"><span style="--sample: var(--danger)"></span><code>--danger</code></div>
    </div>
  </section>

  <div class="showroom-split">
    <section class="showroom-section" aria-labelledby="typography-heading">
      <h2 id="typography-heading" class="showroom-heading">Typography</h2>
      <BevelLine direction="x" length="100%" decorative />
      <div class="type-samples">
        <div><span class="type-route">Workspace identity</span><code>24 / 32</code></div>
        <div><span class="type-body">Repository access</span><code>14 / 20</code></div>
        <div><span class="type-meta">refs/heads/main</span><code>12 / 16</code></div>
      </div>
    </section>

    <section class="showroom-section" aria-labelledby="spacing-heading">
      <h2 id="spacing-heading" class="showroom-heading">Spacing</h2>
      <BevelLine direction="x" length="100%" decorative />
      <div class="space-samples">
        <div><span style="--size: var(--space-1)"></span><code>4</code></div>
        <div><span style="--size: var(--space-2)"></span><code>8</code></div>
        <div><span style="--size: var(--space-3)"></span><code>12</code></div>
        <div><span style="--size: var(--space-4)"></span><code>16</code></div>
        <div><span style="--size: var(--space-5)"></span><code>24</code></div>
        <div><span style="--size: var(--space-6)"></span><code>32</code></div>
      </div>
    </section>
  </div>
</div>

<script lang="ts">
  import Bevel from '$lib/workspace/ui/Bevel.svelte';
  import BevelLine from '$lib/workspace/ui/BevelLine.svelte';
  import Tooltip from '$lib/workspace/ui/Tooltip.svelte';

  function keepSampleFormLocal(event: SubmitEvent) {
    event.preventDefault();
  }
</script>

<div class="showroom showroom--settings">
  <section class="showroom-section" aria-labelledby="form-field-heading">
    <h1 id="form-field-heading" class="showroom-heading">Form field</h1>
    <BevelLine direction="x" length="100%" decorative />
    <form class="sample-form" onsubmit={keepSampleFormLocal}>
      <div class="form-grid">
        <label class="form-field">
          <span>Runtime label</span>
          <Bevel fill>
            <input name="runtime-label" value="Remote Linux runner" />
          </Bevel>
        </label>

        <label class="form-field">
          <span>Endpoint</span>
          <Bevel fill invalid>
            <input
              name="runtime-endpoint"
              value="http://runner.internal.example:8787/api/runtime"
              aria-describedby="endpoint-constraint endpoint-error"
              aria-invalid="true"
            />
          </Bevel>
          <small id="endpoint-constraint">Use an HTTPS endpoint reachable from the Workspace server.</small>
          <span id="endpoint-error" class="field-error">This endpoint uses HTTP. Enter an HTTPS URL.</span>
        </label>

        <label class="form-field">
          <span>Architecture</span>
          <Bevel fill>
            <select name="architecture">
              <option>arm64</option>
              <option selected>x86_64</option>
            </select>
          </Bevel>
        </label>
      </div>

      <BevelLine direction="x" length="100%" decorative />
      <div class="form-footer">
        <p class="unsaved"><span class="status__marker" data-tone="warning"></span>Unsaved changes</p>
        <Bevel>
          <button class="action action--primary" type="submit">Save runtime</button>
        </Bevel>
      </div>
    </form>
  </section>

  <section class="showroom-section" aria-labelledby="operation-error-heading">
    <h2 id="operation-error-heading" class="showroom-heading">Operation error</h2>
    <BevelLine direction="x" length="100%" decorative />
    <div class="feedback feedback--error" role="alert">
      <div>
        <strong>Runtime was not saved.</strong>
        <p>The endpoint did not respond within 10 seconds. Check the address and try again.</p>
      </div>
      <Tooltip id="retry-runtime-help" text="Checks this endpoint again without saving other fields." placement="bottom" align="end">
        {#snippet children(descriptionId)}
          <Bevel>
            <button class="action action--secondary" type="button" aria-describedby={descriptionId}>Test endpoint</button>
          </Bevel>
        {/snippet}
      </Tooltip>
    </div>
  </section>

  <section class="showroom-section" aria-labelledby="permission-boundary-heading">
    <h2 id="permission-boundary-heading" class="showroom-heading">Permission boundary</h2>
    <BevelLine direction="x" length="100%" decorative />
    <div class="permission-read-view">
      <dl class="key-value">
        <div><dt>Credential source</dt><dd>Workspace secret store</dd></div>
        <BevelLine as="div" direction="x" length="100%" decorative />
        <div><dt>Access</dt><dd><span class="status"><span class="status__marker" data-tone="muted"></span>Read-only</span></dd></div>
      </dl>
      <p>Workspace owner permission is required to replace repository credentials.</p>
    </div>
  </section>

  <section class="showroom-section" aria-labelledby="empty-heading">
    <h2 id="empty-heading" class="showroom-heading">Empty</h2>
    <BevelLine direction="x" length="100%" decorative />
    <div class="feedback">
      <strong>No Profile Sources.</strong>
      <Bevel>
        <button class="action action--secondary" type="button">Add profile source</button>
      </Bevel>
    </div>
  </section>
</div>

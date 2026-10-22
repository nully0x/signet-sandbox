<script lang="ts">
  import { api } from '../lib/rpc'

  let { onCreated, onCancel }: { onCreated(name: string): void; onCancel(): void } = $props()

  let name = $state('')
  let ttl = $state('20m')
  let explorer = $state(true)
  let faucet = $state(true)
  let wantIndexer = $state(false)
  let instances = $state<string[]>([])
  let newInstance = $state('')
  let error = $state('')
  let submitting = $state(false)

  const DNS_LABEL = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/
  const TTL = /^[0-9]+[smhd]$/

  // Lightning needs the indexer: the server forces it on, the form
  // mirrors that instead of letting the user think it is optional.
  const indexer = $derived(instances.length > 0 || wantIndexer)
  const invalidInstances = $derived(
    instances.filter((n) => !DNS_LABEL.test(n)),
  )
  const formValid = $derived(
    DNS_LABEL.test(name) && TTL.test(ttl) && invalidInstances.length === 0,
  )

  function parseTtlSeconds(raw: string): number | null {
    const match = raw.match(/^([0-9]+)([smhd])$/)
    if (!match) return null
    const unit = { s: 1, m: 60, h: 3600, d: 86400 }[match[2] as 's' | 'm' | 'h' | 'd']
    return Number(match[1]) * unit
  }

  function addInstance() {
    const candidate = newInstance.trim().toLowerCase()
    if (!DNS_LABEL.test(candidate) || instances.includes(candidate)) return
    instances = [...instances, candidate]
    newInstance = ''
  }

  function removeInstance(target: string) {
    instances = instances.filter((n) => n !== target)
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (!formValid || submitting) return
    submitting = true
    error = ''
    try {
      await api.create({
        name: name.trim(),
        ttl_secs: parseTtlSeconds(ttl.trim()),
        components: {
          indexer,
          faucet,
          explorer,
          lightning: instances.map((n) => ({ name: n, implementation: 'lnd' })),
        },
      })
      onCreated(name.trim())
    } catch (e) {
      error = e instanceof Error ? e.message : 'create failed'
    } finally {
      submitting = false
    }
  }
</script>

<form class="panel" onsubmit={submit}>
  <h2 class="panel-title">new environment — draft</h2>

  <div class="field">
    <label for="f-name">name</label>
    <input id="f-name" class="input" bind:value={name} placeholder="my-env" spellcheck="false" />
    <span class="hint">dns label: lowercase letters, digits, dashes</span>
  </div>

  <div class="field">
    <label for="f-ttl">ttl</label>
    <input id="f-ttl" class="input" bind:value={ttl} placeholder="20m" spellcheck="false" />
    <span class="hint">seconds, minutes, hours, days — e.g. 45s, 20m, 6h, 2d</span>
  </div>

  <div class="field">
    <span class="field-label">components</span>
    <label class="check"><input type="checkbox" bind:checked={wantIndexer} disabled={instances.length > 0} /> indexer (electrs)</label>
    <label class="check"><input type="checkbox" bind:checked={faucet} /> faucet</label>
    <label class="check"><input type="checkbox" bind:checked={explorer} /> explorer (mempool)</label>
    {#if instances.length > 0}
      <span class="hint">indexer is required by lightning and locked on</span>
    {/if}
  </div>

  <div class="field">
    <label for="f-instance">lightning instances (lnd)</label>
    <div class="add-row">
      <input
        id="f-instance"
        class="input"
        bind:value={newInstance}
        placeholder="ln-alice"
        spellcheck="false"
      />
      <button class="btn" type="button" onclick={addInstance} disabled={!DNS_LABEL.test(newInstance.trim().toLowerCase())}>
        add
      </button>
    </div>
    {#each instances as instance (instance)}
      <div class="instance-row">
        <span class="instance-name">{instance}</span>
        <button class="btn" type="button" onclick={() => removeInstance(instance)}>remove</button>
      </div>
    {/each}
    {#if invalidInstances.length > 0}
      <span class="error-text">invalid instance name: {invalidInstances.join(', ')}</span>
    {/if}
  </div>

  {#if error}
    <p class="error-text">{error}</p>
  {/if}

  <div class="form-actions">
    <button class="btn" type="button" onclick={onCancel} disabled={submitting}>cancel</button>
    <button class="btn btn--primary" type="submit" disabled={!formValid || submitting}>
      {submitting ? 'drafting…' : 'provision environment'}
    </button>
  </div>
</form>

<style>
  .form-actions {
    display: flex;
    gap: 0.6rem;
    justify-content: flex-end;
  }
  .check {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.8rem;
    color: var(--ink);
    text-transform: none;
    letter-spacing: 0;
  }
  .check input {
    accent-color: var(--accent);
  }
  .check:has(input:disabled) {
    color: var(--dim);
  }
  .add-row {
    display: flex;
    gap: 0.5rem;
  }
  .instance-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border: 1px solid var(--line);
    background: var(--panel-deep);
    padding: 0.45rem 0.7rem;
    font-size: 0.8rem;
  }
  .instance-name {
    color: var(--accent);
  }
</style>

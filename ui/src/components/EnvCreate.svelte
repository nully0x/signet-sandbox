<script lang="ts">
  import { api } from '../lib/rpc'
  import { Button, Card, Field, Input } from '../lib/ui'

  let { onCreated, onCancel }: { onCreated(name: string): void; onCancel(): void } = $props()

  let name = $state('')
  let ttl = $state('20m')
  let explorer = $state(true)
  let faucet = $state(true)
  let wantIndexer = $state(false)
  let instances = $state<string[]>([])
  let instanceImpls = $state<Record<string, 'lnd' | 'cln'>>({})
  let newInstance = $state('')
  let error = $state('')
  let submitting = $state(false)

  const DNS_LABEL = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/
  const TTL = /^[0-9]+[smhd]$/

  // Lightning does not need the indexer; electrs is an opt-in
  // component for wallets that track on-chain state via Electrum.
  const indexer = $derived(wantIndexer)
  const invalidInstances = $derived(
    instances.filter((n) => !DNS_LABEL.test(n)),
  )
  const formValid = $derived(
    DNS_LABEL.test(name) &&
      (!ttl.trim() || TTL.test(ttl.trim())) &&
      invalidInstances.length === 0,
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
    instanceImpls = { ...instanceImpls, [candidate]: 'lnd' }
    newInstance = ''
  }

  function removeInstance(target: string) {
    instances = instances.filter((n) => n !== target)
    const { [target]: _removed, ...rest } = instanceImpls
    instanceImpls = rest
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (!formValid || submitting) return
    submitting = true
    error = ''
    try {
      await api.create({
        name: name.trim(),
        ttl_secs: ttl.trim() ? parseTtlSeconds(ttl.trim()) : null,
        components: {
          indexer,
          faucet,
          explorer,
          lightning: instances.map((n) => ({
            name: n,
            implementation: instanceImpls[n] ?? 'lnd',
          })),
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

<form onsubmit={submit}>
  <Card title="new environment — draft">
    <Field label="name" for="f-name" hint="dns label: lowercase letters, digits, dashes">
      <Input id="f-name" bind:value={name} placeholder="my-env" spellcheck="false" />
    </Field>

    <Field label="ttl" for="f-ttl" hint="seconds, minutes, hours, days — e.g. 45s, 20m, 6h, 2d; empty = no expiry">
      <Input id="f-ttl" bind:value={ttl} placeholder="20m" spellcheck="false" />
    </Field>

    <Field label="components">
      <label class="check"><input type="checkbox" bind:checked={wantIndexer} /> indexer (electrs)</label>
      <label class="check"><input type="checkbox" bind:checked={faucet} /> faucet</label>
      <label class="check"><input type="checkbox" bind:checked={explorer} /> explorer (mempool)</label>
    </Field>

    <Field label="lightning instances" for="f-instance">
      <div class="add-row">
        <Input
          id="f-instance"
          bind:value={newInstance}
          placeholder="ln-alice"
          spellcheck="false"
        />
        <Button
          onclick={addInstance}
          disabled={!DNS_LABEL.test(newInstance.trim().toLowerCase())}
        >
          add
        </Button>
      </div>
      <div class="rows">
        {#each instances as instance (instance)}
          <div class="instance-row">
            <span class="instance-name">{instance}</span>
            <select
              class="impl-select"
              aria-label="implementation for {instance}"
              bind:value={instanceImpls[instance]}
            >
              <option value="lnd">lnd</option>
              <option value="cln">cln</option>
            </select>
            <Button onclick={() => removeInstance(instance)}>remove</Button>
          </div>
        {/each}
        {#if invalidInstances.length > 0}
          <span class="error-text">invalid instance name: {invalidInstances.join(', ')}</span>
        {/if}
      </div>
    </Field>

    {#if error}
      <p class="error-text">{error}</p>
    {/if}

    <div class="form-actions">
      <Button onclick={onCancel} disabled={submitting}>cancel</Button>
      <Button variant="primary" type="submit" disabled={!formValid || submitting}>
        {submitting ? 'drafting…' : 'provision environment'}
      </Button>
    </div>
  </Card>
</form>

<style>
  .check {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.8rem;
    color: var(--ink);
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
  .add-row :global(input) {
    flex: 1;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    margin-top: 0.4rem;
  }
  .instance-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
    border: 1px solid var(--line);
    background: var(--panel-deep);
    border-radius: var(--radius);
    padding: 0.45rem 0.7rem;
    font-size: 0.8rem;
  }
  .impl-select {
    font: inherit;
    font-size: 0.8rem;
    background: var(--panel);
    border: 1px solid var(--line-strong);
    color: var(--ink);
    border-radius: var(--radius);
    padding: 0.2rem 0.4rem;
  }
  .instance-name {
    color: var(--accent);
  }
  .form-actions {
    display: flex;
    gap: 0.6rem;
    justify-content: flex-end;
  }
</style>

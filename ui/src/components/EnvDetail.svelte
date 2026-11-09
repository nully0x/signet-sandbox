<script lang="ts">
  import { api, type Bundle, RpcError, UNAUTHENTICATED } from '../lib/rpc'
  import Copyable from './Copyable.svelte'
  import StatusPill from './StatusPill.svelte'

  let { envRef, back }: { envRef: string; back(): void } = $props()

  let bundle = $state<Bundle | null>(null)
  let error = $state('')
  let loading = $state(false)

  let actionBusy = $state(false)
  let confirmName = $state('')
  let confirmingDestroy = $state(false)
  let confirmingDelete = $state(false)

  let fundAddress = $state('')
  let fundAmount = $state<number | null>(null)
  let fundBusy = $state(false)
  let fundError = $state('')
  let fundTxid = $state('')

  let lnName = $state('')
  let lnBusy = $state(false)
  let lnError = $state('')
  let pendingNode = $state('')

  const canStop = $derived(bundle?.status === 'ready')
  const canStart = $derived(bundle?.status === 'stopped')
  const canDestroy = $derived(
    bundle !== null && !['destroyed', 'expired'].includes(bundle.status),
  )
  const canDelete = $derived(
    bundle !== null && ['destroyed', 'expired'].includes(bundle.status),
  )
  const destroyArmed = $derived(confirmingDestroy && confirmName === envRef)
  const hasFaucet = $derived(bundle?.faucet_url !== null && bundle?.faucet_url !== undefined)

  async function load() {
    loading = true
    error = ''
    try {
      bundle = await api.get(envRef)
    } catch (e) {
      if (e instanceof RpcError && e.code === UNAUTHENTICATED) {
        back()
        return
      }
      error = e instanceof Error ? e.message : 'request failed'
    } finally {
      loading = false
    }
  }

  // Drafting-table behaviour: while the machine works, the sheet redraws
  // itself — poll until the environment settles.
  $effect(() => {
    bundle = null
    load()
    return () => clearInterval(pollTimer)
  })

  let pollTimer: ReturnType<typeof setInterval> | undefined = $state()
  $effect(() => {
    const status = bundle?.status
    // Keep redrawing while work is in flight: provisioning, or a newly
    // added lightning node whose wallet has not surfaced in the bundle.
    const waiting =
      pendingNode !== '' && !(pendingNode in (bundle?.lightning ?? {}))
    clearInterval(pollTimer)
    if (status === 'provisioning' || status === 'starting' || waiting) {
      pollTimer = setInterval(load, 3000)
      return () => clearInterval(pollTimer)
    }
  })

  $effect(() => {
    if (pendingNode !== '' && bundle?.lightning?.[pendingNode]) {
      pendingNode = ''
    }
  })

  async function lifecycle(kind: 'stop' | 'start' | 'destroy') {
    if (actionBusy || !bundle) return
    actionBusy = true
    error = ''
    try {
      if (kind === 'stop') await api.stop(envRef)
      else if (kind === 'start') await api.start(envRef)
      else {
        await api.destroy(envRef)
        back()
        return
      }
      await load()
    } catch (e) {
      if (e instanceof RpcError && e.code === UNAUTHENTICATED) {
        back()
        return
      }
      error = e instanceof Error ? e.message : `${kind} failed`
    } finally {
      actionBusy = false
    }
  }

  function toggleConfirmDestroy() {
    confirmingDestroy = !confirmingDestroy
    confirmName = ''
  }

  function toggleConfirmDelete() {
    confirmingDelete = !confirmingDelete
  }

  async function purge() {
    if (actionBusy || !bundle) return
    actionBusy = true
    error = ''
    try {
      await api.delete(envRef)
      back()
    } catch (e) {
      if (e instanceof RpcError && e.code === UNAUTHENTICATED) {
        back()
        return
      }
      error = e instanceof Error ? e.message : 'delete failed'
      actionBusy = false
    }
  }

  async function fund(event: SubmitEvent) {
    event.preventDefault()
    if (fundBusy || !fundAddress || !fundAmount || fundAmount <= 0) return
    fundBusy = true
    fundError = ''
    fundTxid = ''
    try {
      const result = await api.fund(envRef, fundAddress.trim(), fundAmount)
      fundTxid = result.txid
      fundAddress = ''
      fundAmount = null
    } catch (e) {
      fundError = e instanceof Error ? e.message : 'faucet failed'
    } finally {
      fundBusy = false
    }
  }

  const instances = $derived(Object.entries(bundle?.lightning ?? {}))

  function dnsLabelOk(name: string): boolean {
    // Instance names become k8s resource names: dns-1123 labels.
    return /^[a-z0-9][a-z0-9-]{0,62}$/.test(name)
  }

  async function addLightning(event: SubmitEvent) {
    event.preventDefault()
    const name = lnName.trim()
    if (lnBusy || !name || !bundle) return
    if (!dnsLabelOk(name)) {
      lnError = 'name must be a dns label: lowercase letters, digits, hyphens; max 63 chars'
      return
    }
    if (instances.some(([existing]) => existing === name)) {
      lnError = `node “${name}” already exists`
      return
    }
    lnBusy = true
    lnError = ''
    try {
      const desired = [
        ...instances.map(([existing, ln]) => ({
          name: existing,
          implementation: ln.implementation,
        })),
        { name, implementation: 'lnd' },
      ]
      bundle = await api.update(envRef, desired)
      pendingNode = name
      lnName = ''
    } catch (e) {
      lnError = e instanceof Error ? e.message : 'update failed'
    } finally {
      lnBusy = false
    }
  }
</script>

{#if loading && !bundle}
  <p class="note">loading {envRef} …</p>
{:else if error && !bundle}
  <p class="error-text">{error} <button class="btn" onclick={back}>back</button></p>
{:else if bundle}
  <header>
    <button class="btn" onclick={back}>← registry</button>
    <h1>{envRef}</h1>
    <StatusPill status={bundle.status} />
  </header>

  <div class="panel actions">
    <button class="btn" onclick={() => lifecycle('stop')} disabled={!canStop || actionBusy}>stop</button>
    <button class="btn" onclick={() => lifecycle('start')} disabled={!canStart || actionBusy}>start</button>
    {#if canDestroy}
      <button class="btn btn--danger" onclick={toggleConfirmDestroy} disabled={actionBusy}>
        {confirmingDestroy ? 'cancel' : 'destroy'}
      </button>
    {/if}
    {#if confirmingDestroy && canDestroy}
      <span class="confirm-cluster">
        <span class="note">type “{envRef}” to confirm destruction</span>
        <input class="input confirm-input" bind:value={confirmName} placeholder={envRef} spellcheck="false" />
        <button
          class="btn btn--danger confirm"
          onclick={() => lifecycle('destroy')}
          disabled={!destroyArmed || actionBusy}
        >
          confirm destroy
        </button>
      </span>
    {/if}
    {#if canDelete}
      <button class="btn btn--danger" onclick={toggleConfirmDelete} disabled={actionBusy}>
        {confirmingDelete ? 'cancel' : 'delete record'}
      </button>
    {/if}
    {#if confirmingDelete && canDelete}
      <span class="confirm-cluster">
        <span class="note">removes the record; the cluster is already gone</span>
        <button class="btn btn--danger confirm" onclick={purge} disabled={actionBusy}>
          confirm delete
        </button>
      </span>
    {/if}
    {#if error}
      <span class="error-text">{error}</span>
    {/if}
  </div>

  <section class="panel">
    <h2 class="panel-title">endpoints</h2>
    <Copyable label="rpc url" value={bundle.rpc_url} />
    {#if bundle.rpc_auth}
      <Copyable label="rpc auth" value={bundle.rpc_auth} secret />
    {/if}
    {#if bundle.indexer_endpoint}
      <Copyable label="electrum" value={bundle.indexer_endpoint} />
    {/if}
    {#if bundle.explorer_url}
      <Copyable label="explorer" value={bundle.explorer_url} href={bundle.explorer_url} />
    {/if}
    {#if bundle.faucet_url}
      <Copyable label="faucet" value={bundle.faucet_url} />
    {/if}
  </section>

  {#if hasFaucet && bundle.status === 'ready'}
    <form class="panel" onsubmit={fund}>
      <h2 class="panel-title">faucet — mint signet coin</h2>
      <div class="fund-grid">
        <label class="field">
          <span>address</span>
          <input class="input" bind:value={fundAddress} placeholder="sb1q…" spellcheck="false" />
        </label>
        <label class="field">
          <span>amount (sats)</span>
          <input class="input" type="number" min="1" bind:value={fundAmount} placeholder="1000000" />
        </label>
        <button class="btn btn--primary" type="submit" disabled={fundBusy || !fundAddress || !fundAmount}>
          {fundBusy ? 'minting…' : 'fund'}
        </button>
      </div>
      {#if fundError}
        <p class="error-text">{fundError}</p>
      {/if}
      {#if fundTxid}
        <Copyable label="txid" value={fundTxid} />
      {/if}
    </form>
  {/if}

  <section class="panel">
    <h2 class="panel-title">chain</h2>
    <Copyable label="challenge" value={bundle.signet_challenge} />
    <Copyable label="block policy" value={bundle.block_policy} />
    {#if bundle.expires_at}
      <Copyable label="expires" value={new Date(bundle.expires_at).toLocaleString()} />
    {/if}
  </section>

  {#if bundle.versions}
    <section class="panel">
      <h2 class="panel-title">versions</h2>
      {#each Object.entries(bundle.versions) as [component, tag] (component)}
        <Copyable label={component} value={tag} />
      {/each}
    </section>
  {/if}

  <section class="panel">
    <h2 class="panel-title">lightning</h2>
    {#if instances.length === 0 && bundle.status !== 'ready'}
      <p class="note">no lightning instances on this sheet — the environment must be ready to add one</p>
    {/if}
    {#if bundle.status === 'ready'}
      <form class="ln-grid" onsubmit={addLightning}>
        <label class="field">
          <span>new node name</span>
          <input class="input" bind:value={lnName} placeholder="ln-carol" spellcheck="false" />
        </label>
        <label class="field">
          <span>implementation</span>
          <input class="input" value="lnd" disabled />
        </label>
        <button class="btn btn--primary" type="submit" disabled={lnBusy || !lnName.trim()}>
          {lnBusy ? 'adding…' : 'add node' }
        </button>
      </form>
      {#if lnError}
        <p class="error-text">{lnError}</p>
      {/if}
      {#if pendingNode}
        <p class="note">waiting for “{pendingNode}” — its card appears here when the wallet is ready</p>
      {/if}
    {/if}
    {#each instances as [name, ln] (name)}
      <div class="instance">
        <h3>{name} <span class="impl">{ln.implementation}</span></h3>
        <Copyable label="node id" value={ln.node_id} />
        <Copyable label="peer uri" value={ln.node_uri} />
        <Copyable label="rest url" value={ln.rest_url} />
        <Copyable label="grpc url" value={ln.grpc_url} />
        <Copyable label="macaroon" value={ln.auth.value} secret />
        <Copyable label="tls ca (hex)" value={ln.tls_cert} secret />
      </div>
    {/each}
  </section>
{/if}

<style>
  header {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 1.2rem;
  }
  h1 {
    font-size: 1.5rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-right: auto;
  }
  .actions {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .confirm-cluster {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    flex-wrap: wrap;
    border-left: 1px solid var(--line-strong);
    padding-left: 0.9rem;
  }
  .confirm-input {
    width: 12rem;
  }
  .fund-grid {
    display: grid;
    grid-template-columns: 1fr 10rem auto;
    gap: 0.9rem;
    align-items: end;
  }
  .fund-grid .field {
    margin-bottom: 0;
  }
  .ln-grid {
    display: grid;
    grid-template-columns: 1fr 10rem auto;
    gap: 0.9rem;
    align-items: end;
    margin-bottom: 0.4rem;
  }
  .ln-grid .field {
    margin-bottom: 0;
  }
  .instance {
    border: 1px solid var(--line);
    background: var(--panel-deep);
    padding: 0.4rem 1rem 0.7rem;
    margin: 0.6rem 0;
  }
  h3 {
    font-size: 1rem;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    margin: 0.6rem 0 0.3rem;
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
  }
  .impl {
    color: var(--accent);
    font-size: 0.65rem;
    letter-spacing: 0.16em;
  }
</style>

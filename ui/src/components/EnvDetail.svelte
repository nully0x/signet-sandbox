<script lang="ts">
  import { api, type Bundle, RpcError, UNAUTHENTICATED } from '../lib/rpc'
  import { Badge, Button, Card, Copyable, EmptyState, Field, Input, Spinner, Tabs, statusTone } from '../lib/ui'

  let { envRef, back, touch }: { envRef: string; back(): void; touch(): void } = $props()

  let bundle = $state<Bundle | null>(null)
  let error = $state('')
  let loading = $state(false)

  let tab = $state('overview')
  const TABS = [
    { id: 'overview', label: 'overview' },
    { id: 'connect', label: 'connect' },
    { id: 'chain', label: 'chain' },
    { id: 'lightning', label: 'lightning' },
  ]

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

  // Status seen in the previous bundle; flips trigger a rail touch.
  let lastStatus = ''

  async function load() {
    loading = true
    error = ''
    try {
      bundle = await api.get(envRef)
      if (bundle.status !== lastStatus) {
        lastStatus = bundle.status
        touch()
      }
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

  // Drafting-table behaviour: while the machine works, the view redraws
  // itself — poll until the environment settles.
  let pollTimer: ReturnType<typeof setInterval> | undefined

  // A node exec-checked mid-boot is omitted from the bundle; keep
  // polling for this window after ready so late nodes surface as cards.
  const READY_GRACE_MS = 15000
  let readyAt = 0

  $effect(() => {
    bundle = null
    load()
    return () => clearInterval(pollTimer)
  })

  $effect(() => {
    const status = bundle?.status
    // Keep redrawing while work is in flight: provisioning, a newly
    // added lightning node whose wallet has not surfaced, or the grace
    // window right after ready.
    const waiting = pendingNode !== '' && !(pendingNode in (bundle?.lightning ?? {}))
    if (status === 'ready') {
      if (readyAt === 0) readyAt = Date.now()
    } else {
      readyAt = 0
    }
    const inGrace = readyAt !== 0 && Date.now() - readyAt < READY_GRACE_MS
    clearInterval(pollTimer)
    if (status === 'provisioning' || status === 'starting' || waiting || inGrace) {
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
        touch()
        back()
        return
      }
      touch()
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
      touch()
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
  <p class="note loading"><Spinner size="0.8rem" /> loading {envRef} …</p>
{:else if error && !bundle}
  <p class="error-text">{error} <Button onclick={back}>back</Button></p>
{:else if bundle}
  <header>
    <Button onclick={back}>← registry</Button>
    <h1>{envRef}</h1>
    <Badge tone={statusTone(bundle.status)} dot={statusTone(bundle.status) === 'busy'}>
      {bundle.status}
    </Badge>
  </header>

  <Tabs tabs={TABS} active={tab} onchange={(id) => (tab = id)} />

  {#if tab === 'overview'}
    <div class="tabpanel" id="panel-overview" role="tabpanel" aria-labelledby="tab-overview">
      <Card title="lifecycle">
        <div class="actions">
          <Button onclick={() => lifecycle('stop')} disabled={!canStop || actionBusy}>stop</Button>
          <Button onclick={() => lifecycle('start')} disabled={!canStart || actionBusy}>start</Button>
          {#if canDestroy}
            <Button variant="danger" onclick={toggleConfirmDestroy} disabled={actionBusy}>
              {confirmingDestroy ? 'cancel' : 'destroy'}
            </Button>
          {/if}
          {#if confirmingDestroy && canDestroy}
            <span class="confirm-cluster">
              <span class="note">type “{envRef}” to confirm destruction</span>
              <Input bind:value={confirmName} placeholder={envRef} spellcheck="false" />
              <Button
                variant="danger"
                filled={destroyArmed}
                onclick={() => lifecycle('destroy')}
                disabled={!destroyArmed || actionBusy}
              >
                confirm destroy
              </Button>
            </span>
          {/if}
          {#if canDelete}
            <Button variant="danger" onclick={toggleConfirmDelete} disabled={actionBusy}>
              {confirmingDelete ? 'cancel' : 'delete record'}
            </Button>
          {/if}
          {#if confirmingDelete && canDelete}
            <span class="confirm-cluster">
              <span class="note">removes the record; the cluster is already gone</span>
              <Button variant="danger" filled onclick={purge} disabled={actionBusy}>
                confirm delete
              </Button>
            </span>
          {/if}
          {#if error}
            <span class="error-text">{error}</span>
          {/if}
        </div>
        {#if bundle.status === 'provisioning' || bundle.status === 'starting'}
          <p class="note">the machine is working — this view redraws until the environment settles</p>
        {/if}
        {#if bundle.expires_at}
          <div class="kv">
            <span>expires</span>
            <code class="tnum">{new Date(bundle.expires_at).toLocaleString()}</code>
          </div>
        {/if}
      </Card>

      {#if bundle.versions && Object.keys(bundle.versions).length > 0}
        <Card title="versions">
          {#each Object.entries(bundle.versions) as [component, tag] (component)}
            <div class="kv">
              <span>{component}</span>
              <code>{tag}</code>
            </div>
          {/each}
        </Card>
      {/if}
    </div>
  {:else if tab === 'connect'}
    <div class="tabpanel" id="panel-connect" role="tabpanel" aria-labelledby="tab-connect">
      <Card title="api">
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
      </Card>
    </div>
  {:else if tab === 'chain'}
    <div class="tabpanel" id="panel-chain" role="tabpanel" aria-labelledby="tab-chain">
      <Card title="chain">
        <Copyable label="challenge" value={bundle.signet_challenge} />
        <Copyable label="block policy" value={bundle.block_policy} />
      </Card>

      {#if hasFaucet && bundle.status === 'ready'}
        <Card title="faucet — mint signet coin">
          <form class="fund-grid" onsubmit={fund}>
            <Field label="address">
              <Input bind:value={fundAddress} placeholder="sb1q…" spellcheck="false" />
            </Field>
            <Field label="amount (sats)">
              <Input type="number" min="1" bind:value={fundAmount} placeholder="1000000" />
            </Field>
            <Button variant="primary" type="submit" disabled={fundBusy || !fundAddress || !fundAmount}>
              {fundBusy ? 'minting…' : 'fund'}
            </Button>
          </form>
          {#if fundError}
            <p class="error-text">{fundError}</p>
          {/if}
          {#if fundTxid}
            <Copyable label="txid" value={fundTxid} />
          {/if}
        </Card>
      {/if}
    </div>
  {:else}
    <div class="tabpanel" id="panel-lightning" role="tabpanel" aria-labelledby="tab-lightning">
      {#if bundle.status === 'ready'}
        <Card title="add node">
          <form class="ln-grid" onsubmit={addLightning}>
            <Field label="new node name">
              <Input bind:value={lnName} placeholder="ln-carol" spellcheck="false" />
            </Field>
            <Field label="implementation">
              <Input value="lnd" disabled />
            </Field>
            <Button variant="primary" type="submit" disabled={lnBusy || !lnName.trim()}>
              {lnBusy ? 'adding…' : 'add node'}
            </Button>
          </form>
          {#if lnError}
            <p class="error-text">{lnError}</p>
          {/if}
          {#if pendingNode}
            <p class="note">waiting for “{pendingNode}” — its card appears here when the wallet is ready</p>
          {/if}
        </Card>
      {/if}

      {#if instances.length === 0}
        <EmptyState>
          <p>
            {bundle.status === 'ready'
              ? 'no lightning instances yet — add one above'
              : 'no lightning instances on this environment — it must be ready to add one'}
          </p>
        </EmptyState>
      {:else}
        <div class="nodes">
          {#each instances as [name, ln] (name)}
            <div class="node">
              <header>
                <h3>{name}</h3>
                <Badge tone="accent">{ln.implementation}</Badge>
              </header>
              <Copyable label="node id" value={ln.node_id} />
              <Copyable label="peer uri" value={ln.node_uri} />
              <Copyable label="rest url" value={ln.rest_url} />
              {#if ln.grpc_url}
                <Copyable label="grpc url" value={ln.grpc_url} />
              {/if}
              {#if ln.auth.type === 'rune'}
                <Copyable label="rune" value={ln.auth.value} secret />
              {:else}
                <Copyable label="macaroon" value={ln.auth.value} secret />
              {/if}
              <Copyable label="tls ca (hex)" value={ln.tls_cert} secret />
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
{/if}

<style>
  .loading {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  header {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 1rem;
  }
  h1 {
    font-size: 1.4rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-right: auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    border-left: 2px solid var(--line-strong);
    padding-left: 0.9rem;
  }
  .confirm-cluster :global(input) {
    width: 12rem;
  }
  .kv {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    font-size: 0.8rem;
    padding: 0.35rem 0;
    border-bottom: 1px dotted var(--line);
  }
  .kv:last-child {
    border-bottom: none;
  }
  .kv span {
    color: var(--dim);
  }
  .fund-grid,
  .ln-grid {
    display: grid;
    grid-template-columns: 1fr 10rem auto;
    gap: 0.9rem;
    align-items: end;
  }
  .fund-grid :global(input),
  .ln-grid :global(input) {
    width: 100%;
  }
  .nodes {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(24rem, 1fr));
    gap: 0.8rem;
  }
  .node {
    border: 1px solid var(--line);
    background: var(--panel-deep);
    border-radius: var(--radius);
    padding: 0.6rem 1rem 0.7rem;
  }
  .node header {
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
    margin: 0.4rem 0 0.3rem;
  }
  .node h3 {
    font-size: 1rem;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  @media (max-width: 760px) {
    .fund-grid,
    .ln-grid {
      grid-template-columns: 1fr;
    }
    .nodes {
      grid-template-columns: 1fr;
    }
  }
</style>

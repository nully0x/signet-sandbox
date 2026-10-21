<script lang="ts">
  import { api, type Bundle, RpcError, UNAUTHENTICATED } from '../lib/rpc'
  import Copyable from './Copyable.svelte'
  import StatusPill from './StatusPill.svelte'

  let { envRef, back }: { envRef: string; back(): void } = $props()

  let bundle = $state<Bundle | null>(null)
  let error = $state('')
  let loading = $state(false)

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

  $effect(() => {
    bundle = null
    load()
  })

  const instances = $derived(Object.entries(bundle?.lightning ?? {}))
</script>

{#if loading && !bundle}
  <p class="note pulse">loading {envRef} …</p>
{:else if error}
  <p class="note error">{error} <button class="ghost" onclick={back}>back</button></p>
{:else if bundle}
  <header>
    <button class="ghost" onclick={back}>← environments</button>
    <h1>{envRef}</h1>
    <StatusPill status={bundle.status} />
  </header>

  <section>
    <h2>endpoints</h2>
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

  <section>
    <h2>chain</h2>
    <Copyable label="challenge" value={bundle.signet_challenge} />
    <Copyable label="block policy" value={bundle.block_policy} />
    {#if bundle.expires_at}
      <Copyable label="expires" value={new Date(bundle.expires_at).toLocaleString()} />
    {/if}
  </section>

  {#if bundle.versions}
    <section>
      <h2>versions</h2>
      {#each Object.entries(bundle.versions) as [component, tag] (component)}
        <Copyable label={component} value={tag} />
      {/each}
    </section>
  {/if}

  <section>
    <h2>lightning</h2>
    {#if instances.length === 0}
      <p class="note dim-note">no lightning instances — add one with <code>environment.update</code></p>
    {/if}
    {#each instances as [name, ln] (name)}
      <div class="instance">
        <h3>{name} <span class="impl">{ln.implementation}</span></h3>
        <Copyable label="rest url" value={ln.rest_url} />
        <Copyable label="macaroon" value={ln.auth.value} secret />
        <Copyable label="tls cert" value={ln.tls_cert} secret multiline />
      </div>
    {/each}
  </section>
{/if}

<style>
  header {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 1.4rem;
  }
  h1 {
    font-size: 1.2rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    margin-right: auto;
  }
  section {
    border: 1px solid var(--line);
    background: var(--panel);
    padding: 0.4rem 1rem 0.7rem;
    margin-bottom: 1.2rem;
  }
  h2 {
    color: var(--dim);
    font-size: 0.65rem;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    margin: 0.6rem 0 0.4rem;
  }
  .instance {
    border: 1px solid var(--line);
    padding: 0.4rem 1rem 0.7rem;
    margin: 0.6rem 0;
  }
  h3 {
    font-size: 0.85rem;
    font-weight: 600;
    margin: 0.6rem 0 0.3rem;
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
  }
  .impl {
    color: var(--amber);
    font-size: 0.65rem;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .note {
    color: var(--dim);
    font-size: 0.8rem;
  }
  .dim-note code {
    color: var(--amber);
  }
  .error {
    color: var(--red);
  }
  .pulse {
    animation: blink 1.1s ease-in-out infinite;
  }
  button.ghost {
    font: inherit;
    background: none;
    border: 1px solid var(--line);
    color: var(--dim);
    padding: 0.25rem 0.6rem;
    cursor: pointer;
    font-size: 0.7rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  button.ghost:hover {
    color: var(--ink);
    border-color: var(--dim);
  }
  @keyframes blink {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.35; }
  }
</style>

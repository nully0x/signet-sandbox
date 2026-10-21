<script lang="ts">
  import { api, clearToken, storedToken, tokenFromFragment, type EnvSummary, RpcError, UNAUTHENTICATED } from './lib/rpc'
  import TokenGate from './components/TokenGate.svelte'
  import EnvList from './components/EnvList.svelte'
  import EnvDetail from './components/EnvDetail.svelte'

  import EnvCreate from './components/EnvCreate.svelte'

  let envs = $state<EnvSummary[]>([])
  let loadError = $state('')
  let listVersion = $state(0)

  // A `#token=` fragment (from `signet up --ui`) wins over a stored
  // token and is stripped from the address bar once stored.
  const fragment = tokenFromFragment()
  if (fragment) {
    localStorage.setItem('signet.token', fragment)
    history.replaceState(null, '', location.pathname + location.search)
  }
  let authenticated = $state(fragment !== null || storedToken() !== '')

  // Hash routing: `#/` is the list, `#/env/<name-or-id>` the detail view.
  // Hash routing: `#/` is the registry, `#/new` the draft sheet, and
  // `#/env/<name-or-id>` the detail view.
  let route = $state(currentRoute())

  function currentRoute() {
    if (location.hash === '#/new') return { view: 'new', ref: '' }
    const match = location.hash.match(/^#\/env\/(.+)$/)
    return match ? { view: 'env', ref: decodeURIComponent(match[1]) } : { view: 'list', ref: '' }
  }

  function goNew() {
    location.hash = '#/new'
  }

  function openEnv(ref: string) {
    location.hash = `#/env/${encodeURIComponent(ref)}`
  }

  function backToList() {
    location.hash = '#/'
  }

  window.addEventListener('hashchange', () => {
    route = currentRoute()
  })

  async function refresh() {
    try {
      envs = await api.list()
      loadError = ''
    } catch (e) {
      if (e instanceof RpcError && e.code === UNAUTHENTICATED) {
        clearToken()
        authenticated = false
        return
      }
      loadError = 'api unreachable'
    }
  }

  function unlock() {
    authenticated = true
    listVersion++
  }

  $effect(() => {
    if (authenticated && route.view === 'list') {
      void listVersion
      refresh()
    }
  })
</script>

<div class="frame">
  {#if !authenticated}
    <TokenGate onAuth={unlock} />
  {:else if route.view === 'new'}
    <EnvCreate onCreated={openEnv} />
  {:else if route.view === 'env'}
    <EnvDetail envRef={route.ref} back={backToList} />
  {:else}
    <header>
      <p class="mark">signet <span class="sep">/</span> environments</p>
      <div class="nav">
        <button class="ghost" onclick={goNew}>new environment</button>
        <button class="ghost" onclick={() => listVersion++} disabled={loadError !== ''}>
          refresh
        </button>
      </div>
    </header>
    {#if loadError}
      <p class="note error">{loadError}</p>
    {:else}
      <EnvList envs={envs} open={openEnv} />
    {/if}
    <footer>rpc surface <code>/api/rpc</code></footer>
  {/if}
</div>

<style>
  .frame {
    max-width: 56rem;
    margin: 0 auto;
    padding: 2.5rem 1.5rem 4rem;
  }
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 1.2rem;
  }
  .mark {
    font-size: 0.7rem;
    letter-spacing: 0.3em;
    text-transform: uppercase;
    color: var(--dim);
  }
  .mark .sep {
    color: var(--accent);
  }
  .nav {
    display: flex;
    gap: 0.5rem;
  }
  button.ghost {
    font: inherit;
    background: none;
    border: 1px solid var(--line);
    color: var(--dim);
    padding: 0.3rem 0.7rem;
    cursor: pointer;
    font-size: 0.7rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  button.ghost:hover:not(:disabled) {
    color: var(--ink);
    border-color: var(--dim);
  }
  button.ghost:disabled {
    opacity: 0.4;
    cursor: default;
  }
  footer {
    margin-top: 1.4rem;
    color: var(--dim);
    font-size: 0.65rem;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }
  footer code {
    color: var(--accent);
  }
  .note.error {
    color: var(--red);
    font-size: 0.8rem;
  }
</style>

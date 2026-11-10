<script lang="ts">
  import { api, clearToken, storedToken, tokenFromFragment, type EnvSummary, RpcError, UNAUTHENTICATED } from './lib/rpc'
  import { Button, EmptyState } from './lib/ui'
  import TokenGate from './components/TokenGate.svelte'
  import EnvRail from './components/EnvRail.svelte'
  import EnvCreate from './components/EnvCreate.svelte'
  import EnvDetail from './components/EnvDetail.svelte'

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

  // Hash routing: `#/` is the shell (rail + empty pane), `#/new` the
  // draft sheet, and `#/env/<name-or-id>` the detail pane.
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

  // Detail-pane mutations and status flips call touch so the rail's
  // registry copy never waits for the steady 10-second poll.
  function touch() {
    listVersion++
  }

  function unlock() {
    authenticated = true
    listVersion++
  }

  // The rail is always on screen, so the registry stays live: an
  // immediate refresh plus a steady poll while the shell is mounted.
  $effect(() => {
    if (!authenticated) return
    void listVersion
    refresh()
    const timer = setInterval(refresh, 10000)
    return () => clearInterval(timer)
  })
</script>

{#if !authenticated}
  <TokenGate onAuth={unlock} />
{:else}
  <div class="shell">
    <aside class="rail">
      <p class="mark">signet</p>
      <div class="rail-actions">
        <Button variant="ghost" onclick={goNew}>new environment</Button>
        <Button variant="ghost" onclick={() => listVersion++}>refresh</Button>
      </div>
      <EnvRail {envs} activeRef={route.view === 'env' ? route.ref : ''} open={openEnv} />
      {#if loadError}
        <p class="error-text">{loadError}</p>
      {/if}
      <footer>rpc surface <code>/api/rpc</code></footer>
    </aside>
    <main class="pane">
      {#if route.view === 'new'}
        <EnvCreate onCreated={(name) => { touch(); openEnv(name) }} onCancel={backToList} />
      {:else if route.view === 'env'}
        {#key route.ref}
          <EnvDetail envRef={route.ref} back={backToList} touch={touch} />
        {/key}
      {:else}
        <div class="intro">
          <EmptyState>
            <p>select an environment from the rail,<br />or draft a new one.</p>
            <Button variant="primary" onclick={goNew}>new environment</Button>
          </EmptyState>
        </div>
      {/if}
    </main>
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: 15rem 1fr;
    min-height: 100vh;
  }
  .rail {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    border-right: 1px solid var(--line);
    background: var(--panel-deep);
    padding: 1.1rem 0.9rem;
    position: sticky;
    top: 0;
    height: 100vh;
  }
  .mark {
    margin: 0;
    font-size: 0.7rem;
    letter-spacing: 0.3em;
    text-transform: uppercase;
    color: var(--dim);
  }
  .rail-actions {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .rail-actions > :global(button) {
    width: 100%;
  }
  footer {
    margin-top: 0.3rem;
    color: var(--faint);
    font-size: 0.65rem;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }
  footer code {
    color: var(--accent);
  }
  .pane {
    width: 100%;
    max-width: 72rem;
    margin: 0 auto;
    padding: 2rem 2.5rem 4rem;
  }
  .intro {
    max-width: 36rem;
    margin: 3rem auto 0;
  }
  @media (max-width: 760px) {
    .shell {
      grid-template-columns: 1fr;
    }
    .rail {
      position: static;
      height: auto;
      border-right: none;
      border-bottom: 1px solid var(--line);
    }
    .pane {
      padding: 1.5rem 1.2rem 3rem;
    }
  }
</style>

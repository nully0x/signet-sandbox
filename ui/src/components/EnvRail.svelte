<script lang="ts">
  import type { EnvSummary } from '../lib/rpc'
  import Badge from '../lib/ui/Badge.svelte'
  import { statusTone } from '../lib/ui/tone'

  let { envs, activeRef, open }: { envs: EnvSummary[]; activeRef: string; open(name: string): void } =
    $props()

  function expiry(raw: string | null) {
    if (!raw) return '—'
    const secs = Math.floor((Date.parse(raw) - Date.now()) / 1000)
    if (secs <= 0) return 'expired'
    if (secs < 3600) return `${Math.ceil(secs / 60)}m`
    if (secs < 86400) return `${Math.floor(secs / 3600)}h`
    return `${Math.floor(secs / 86400)}d`
  }
</script>

<nav class="registry" aria-label="environments">
  {#each envs as env (env.id)}
    <button
      class="row"
      class:active={env.name === activeRef || env.id === activeRef}
      aria-current={env.name === activeRef ? 'page' : undefined}
      onclick={() => open(env.name)}
    >
      <span class="name">{env.name}</span>
      <span class="meta">
        <Badge tone={statusTone(env.status)} dot={statusTone(env.status) === 'busy'}>
          {env.status}
        </Badge>
        <span class="ttl tnum">{expiry(env.expires_at)}</span>
      </span>
    </button>
  {:else}
    <p class="none">no environments yet</p>
  {/each}
</nav>

<style>
  .registry {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    flex: 1;
    overflow-y: auto;
    margin: 0 -0.9rem;
    padding: 0 0.9rem;
  }
  .row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    background: none;
    border: none;
    color: var(--dim);
    font: inherit;
    font-size: 0.8rem;
    text-align: left;
    padding: 0.4rem 0.5rem;
    cursor: pointer;
    border-radius: var(--radius);
    transition:
      background var(--ease),
      color var(--ease);
  }
  .row:hover {
    background: var(--hover);
    color: var(--ink);
  }
  .row.active {
    background: var(--hover);
    color: var(--ink);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .name {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-shrink: 0;
  }
  .ttl {
    color: var(--faint);
    font-size: 0.7rem;
  }
  .none {
    color: var(--faint);
    font-size: 0.75rem;
    margin: 0.2rem 0.5rem;
  }
  @media (max-width: 760px) {
    .registry {
      flex-direction: row;
      overflow-x: auto;
      margin: 0;
      padding: 0 0 0.25rem;
    }
    .row {
      width: auto;
      flex-shrink: 0;
    }
    .ttl {
      display: none;
    }
  }
</style>

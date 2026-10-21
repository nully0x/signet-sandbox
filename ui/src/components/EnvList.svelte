<script lang="ts">
  import type { EnvSummary } from '../lib/rpc'
  import StatusPill from './StatusPill.svelte'

  let { envs, open }: { envs: EnvSummary[]; open(name: string): void } = $props()

  function short(id: string) {
    return id.slice(0, 8)
  }

  function expiry(raw: string | null) {
    if (!raw) return '—'
    const secs = Math.floor((Date.parse(raw) - Date.now()) / 1000)
    if (secs <= 0) return 'expired'
    if (secs < 3600) return `${Math.ceil(secs / 60)}m left`
    if (secs < 86400) return `${Math.floor(secs / 3600)}h left`
    return `${Math.floor(secs / 86400)}d left`
  }
</script>

<div class="table">
  <div class="row head">
    <span>name</span><span>id</span><span>status</span><span>ttl</span>
  </div>
  {#each envs as env, i (env.id)}
    <button class="row" style={`animation-delay:${i * 45}ms`} onclick={() => open(env.name)}>
      <span class="name">{env.name}</span>
      <span class="dim">{short(env.id)}</span>
      <span><StatusPill status={env.status} /></span>
      <span class="dim">{expiry(env.expires_at)}</span>
    </button>
  {:else}
    <p class="empty">no environments yet — create one with <code>signet up</code></p>
  {/each}
</div>

<style>
  .table {
    border: 1px solid var(--line);
    background: var(--panel);
  }
  .row {
    display: grid;
    grid-template-columns: 1.4fr 1fr 1fr 1fr;
    gap: 1rem;
    align-items: center;
    width: 100%;
    padding: 0.7rem 1rem;
    text-align: left;
    font: inherit;
    color: inherit;
    background: none;
    border: none;
    border-bottom: 1px solid var(--line);
    animation: fade-in 300ms ease both;
  }
  .row:not(.head):hover {
    background: var(--hover);
    cursor: pointer;
  }
  .head {
    color: var(--dim);
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-size: 0.65rem;
    pointer-events: none;
  }
  .name {
    color: var(--ink);
    font-weight: 500;
  }
  .dim {
    color: var(--dim);
  }
  .empty {
    padding: 1.4rem 1rem;
    color: var(--dim);
    font-size: 0.8rem;
  }
  code {
    color: var(--amber);
  }
  @keyframes fade-in {
    from { opacity: 0; transform: translateY(6px); }
    to { opacity: 1; transform: none; }
  }
</style>

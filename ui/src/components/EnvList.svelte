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
    if (secs < 3600) return `${Math.ceil(secs / 60)}m`
    if (secs < 86400) return `${Math.floor(secs / 3600)}h`
    return `${Math.floor(secs / 86400)}d`
  }
</script>

<div class="table panel">
  <div class="row head">
    <span>name</span><span>id</span><span>state</span><span>ttl</span>
  </div>
  {#each envs as env, i (env.id)}
    <button class="row" style={`animation-delay:${i * 45}ms`} onclick={() => open(env.name)}>
      <span class="name">{env.name}</span>
      <span class="dim">{short(env.id)}</span>
      <span><StatusPill status={env.status} /></span>
      <span class="ttl">{expiry(env.expires_at)}</span>
    </button>
  {:else}
    <p class="empty">registry empty — draft your first environment on the <code>new</code> sheet</p>
  {/each}
</div>

<style>
  .table {
    padding: 0;
    overflow: hidden;
  }
  .row {
    display: grid;
    grid-template-columns: 1.4fr 1fr 1.2fr 0.6fr;
    gap: 1rem;
    align-items: center;
    width: 100%;
    padding: 0.7rem 1.1rem;
    text-align: left;
    font: inherit;
    color: inherit;
    background: none;
    border: none;
    border-bottom: 1px solid var(--line);
    animation: fade-in 300ms ease both;
  }
  .row:last-of-type {
    border-bottom: none;
  }
  .row:not(.head):hover {
    background: var(--hover);
    cursor: pointer;
  }
  .head {
    color: var(--faint);
    text-transform: uppercase;
    letter-spacing: 0.14em;
    font-size: 0.62rem;
    pointer-events: none;
  }
  .name {
    color: var(--ink);
    font-weight: 500;
  }
  .dim,
  .ttl {
    color: var(--dim);
  }
  .empty {
    padding: 1.4rem 1.1rem;
    color: var(--dim);
    font-size: 0.8rem;
    margin: 0;
  }
  .empty code {
    color: var(--accent);
  }
  @keyframes fade-in {
    from { opacity: 0; transform: translateY(6px); }
    to { opacity: 1; transform: none; }
  }
</style>

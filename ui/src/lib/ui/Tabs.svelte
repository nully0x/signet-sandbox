<script lang="ts">
  let {
    tabs,
    active,
    onchange,
  }: {
    tabs: { id: string; label: string }[]
    active: string
    onchange: (id: string) => void
  } = $props()
</script>

<div class="tabs" role="tablist">
  {#each tabs as t (t.id)}
    <button
      role="tab"
      id={`tab-${t.id}`}
      aria-selected={active === t.id}
      aria-controls={`panel-${t.id}`}
      class:on={active === t.id}
      onclick={() => onchange(t.id)}
    >
      {t.label}
    </button>
  {/each}
</div>

<style>
  .tabs {
    display: flex;
    gap: 0.25rem;
    border-bottom: 1px solid var(--line);
    margin-bottom: var(--s-4);
    overflow-x: auto;
  }
  button {
    font: inherit;
    font-size: 0.8rem;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--dim);
    padding: 0.45rem 0.7rem;
    cursor: pointer;
    letter-spacing: 0.04em;
    margin-bottom: -1px;
    white-space: nowrap;
    transition: color var(--ease);
  }
  button:hover {
    color: var(--ink);
  }
  button.on {
    color: var(--ink);
    border-bottom-color: var(--accent);
  }
</style>

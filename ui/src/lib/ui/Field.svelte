<script lang="ts">
  import type { Snippet } from 'svelte'

  let {
    label,
    for: id,
    hint = '',
    children,
  }: {
    label: string
    for?: string
    hint?: string
    children: Snippet
  } = $props()
</script>

<div class="field">
  {#if id}
    <label class="label" for={id}>{label}</label>
  {:else}
    <span class="label">{label}</span>
  {/if}
  {@render children()}
  {#if hint}<span class="hint">{hint}</span>{/if}
</div>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .label {
    font-size: 0.78rem;
    font-weight: 500;
    color: var(--dim);
  }
</style>

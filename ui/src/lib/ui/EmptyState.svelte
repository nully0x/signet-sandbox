<script lang="ts">
  import type { Snippet } from 'svelte'

  let { children }: { children: Snippet } = $props()
</script>

<div class="empty">
  {@render children()}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.8rem;
    border: 1px dashed var(--line);
    border-radius: var(--radius);
    padding: 2rem 1rem;
    text-align: center;
    color: var(--dim);
    font-size: 0.8rem;
  }
</style>

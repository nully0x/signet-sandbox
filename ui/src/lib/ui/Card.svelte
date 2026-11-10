<script lang="ts">
  import type { Snippet } from 'svelte'

  let { title = '', children }: { title?: string; children: Snippet } = $props()
</script>

<section class="card">
  {#if title}<h2 class="title">{title}</h2>{/if}
  <div class="body">
    {@render children()}
  </div>
</section>

<style>
  .card {
    border: 1px solid var(--line);
    background: var(--panel);
    border-radius: var(--radius);
    padding: var(--s-4) 1.1rem;
    margin-bottom: var(--s-4);
  }
  .title {
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--dim);
    margin: 0.1rem 0 0.7rem;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    align-items: stretch;
  }
</style>

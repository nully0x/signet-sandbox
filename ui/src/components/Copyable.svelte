<script lang="ts">
  let { label, value, href = '', secret = false, multiline = false } = $props()

  let revealed = $state(false)
  let copied = $state(false)

  const masked = $derived(secret && !revealed)
  const display = $derived(masked ? '•'.repeat(Math.min(value.length, 24)) : value)

  async function copy() {
    await navigator.clipboard.writeText(value)
    copied = true
    setTimeout(() => (copied = false), 1200)
  }
</script>

<div class="copyable" class:multiline>
  <span class="label">{label}</span>
  <span class="value">
    {#if href}
      <a {href} target="_blank" rel="noreferrer">{display || '—'}</a>
    {:else}
      <span class:text-dim={masked}>{display || '—'}</span>
    {/if}
  </span>
  <span class="actions">
    {#if secret}
      <button class="ghost" onclick={() => (revealed = !revealed)}>
        {revealed ? 'hide' : 'show'}
      </button>
    {/if}
    <button class="ghost" onclick={copy} disabled={!value}>
      {copied ? 'copied' : 'copy'}
    </button>
  </span>
</div>

<style>
  .copyable {
    display: grid;
    grid-template-columns: minmax(9rem, max-content) 1fr auto;
    gap: 0.75rem;
    align-items: baseline;
    padding: 0.5rem 0;
    border-bottom: 1px dotted var(--line);
    font-size: 0.8rem;
  }
  .copyable:last-child {
    border-bottom: none;
  }
  .copyable.multiline .value {
    white-space: pre-wrap;
    word-break: break-all;
  }
  .label {
    color: var(--dim);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    font-size: 0.65rem;
    padding-top: 0.15rem;
  }
  .value {
    word-break: break-all;
  }
  .value a {
    color: var(--accent);
    text-decoration: none;
    border-bottom: 1px dotted var(--line-strong);
  }
  .value a:hover {
    color: var(--accent);
  }
  .text-dim {
    color: var(--dim);
    letter-spacing: 0.2em;
  }
  .actions {
    display: flex;
    gap: 0.35rem;
    opacity: 0;
    transition: opacity 120ms ease;
  }
  .copyable:hover .actions,
  .copyable:focus-within .actions {
    opacity: 1;
  }
</style>

<script lang="ts">
  import type { Snippet } from 'svelte'

  let {
    variant = 'default',
    filled = false,
    type = 'button',
    disabled = false,
    onclick,
    children,
  }: {
    variant?: 'default' | 'primary' | 'danger' | 'ghost'
    /** danger only: filled state for an armed destructive confirm */
    filled?: boolean
    type?: 'button' | 'submit'
    disabled?: boolean
    onclick?: (event: MouseEvent) => void
    children: Snippet
  } = $props()
</script>

<button {type} {disabled} {onclick} class={variant} class:filled>
  {@render children()}
</button>

<style>
  button {
    font: inherit;
    font-size: 0.85rem;
    padding: 0.4rem 0.85rem;
    cursor: pointer;
    border: 1px solid var(--line-strong);
    background: var(--panel);
    color: var(--ink);
    border-radius: var(--radius);
    transition:
      background var(--ease),
      border-color var(--ease),
      color var(--ease);
  }
  button:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  button:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-ink);
  }
  .primary:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 85%, #fff);
    border-color: transparent;
    color: var(--accent-ink);
  }

  .danger {
    color: var(--red);
    border-color: var(--red);
  }
  .danger:hover:not(:disabled) {
    background: var(--red);
    border-color: var(--red);
    color: #fff;
  }
  .danger.filled,
  .danger.filled:hover:not(:disabled) {
    background: var(--red);
    border-color: var(--red);
    color: #fff;
  }

  .ghost {
    background: none;
    border-color: var(--line);
    color: var(--dim);
    font-size: 0.7rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: 0.3rem 0.7rem;
  }
  .ghost:hover:not(:disabled) {
    color: var(--ink);
    border-color: var(--dim);
  }
</style>

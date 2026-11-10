<script lang="ts">
  import type { Snippet } from 'svelte'
  import { TONE_COLOR, type Tone } from './tone'

  let {
    tone = 'neutral',
    dot = false,
    children,
  }: {
    tone?: Tone
    dot?: boolean
    children: Snippet
  } = $props()
</script>

<span class="badge" class:busy={tone === 'busy'} style={`--tone: ${TONE_COLOR[tone]}`}>
  {#if dot}<i class="dot" aria-hidden="true"></i>{/if}
  {@render children()}
</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.72rem;
    font-weight: 500;
    padding: 0.1rem 0.5rem;
    border-radius: var(--radius);
    color: var(--tone);
    background: color-mix(in srgb, var(--tone) 12%, transparent);
    white-space: nowrap;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--tone);
  }
  .busy .dot {
    animation: pulse 1.4s ease-in-out infinite;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.35;
    }
  }
</style>

<script lang="ts">
  let { status, stamp = false }: { status: string; stamp?: boolean } = $props()

  const LIVE = ['ready', 'provisioning', 'stopped', 'starting']
  const klass = $derived(LIVE.includes(status) ? status : 'dead')
</script>

<span class="stamp-ink {klass}" class:stamp>
  <span class="led"></span>
  {status}
</span>

<style>
  .stamp-ink {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    font-family: var(--font-display);
    font-size: 0.78rem;
    font-weight: 700;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    padding: 0.16rem 0.6rem 0.1rem;
    border: 1.5px solid currentcolor;
    border-radius: var(--radius);
    color: var(--dim);
    white-space: nowrap;
  }
  .stamp {
    transform: rotate(-2deg);
  }
  .led {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentcolor;
  }
  .ready {
    color: var(--green);
  }
  .provisioning,
  .starting {
    color: var(--amber);
  }
  .provisioning .led,
  .starting .led {
    animation: drafting-blink 1.2s ease-in-out infinite;
  }
  .stopped {
    color: var(--dim);
  }
  .dead {
    color: var(--red);
  }
</style>

<script lang="ts">
  let { status } = $props()

  const LIVE = ['ready', 'provisioning', 'stopped', 'starting']
  const klass = $derived(
    LIVE.includes(status) ? status : 'dead',
  )
</script>

<span class="pill {klass}">
  <span class="led"></span>
  {status}
</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.65rem;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--line);
    color: var(--dim);
  }
  .led {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--dim);
  }
  .ready .led {
    background: var(--green);
    box-shadow: 0 0 6px var(--green);
  }
  .ready {
    color: var(--green);
    border-color: color-mix(in srgb, var(--green) 35%, transparent);
  }
  .provisioning .led,
  .starting .led {
    background: var(--amber);
    animation: blink 1.1s ease-in-out infinite;
  }
  .provisioning,
  .starting {
    color: var(--amber);
    border-color: color-mix(in srgb, var(--amber) 35%, transparent);
  }
  .stopped .led {
    background: var(--dim);
  }
  .dead .led {
    background: var(--red);
  }
  .dead {
    color: var(--red-dim);
  }
  @keyframes blink {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.25; }
  }
</style>

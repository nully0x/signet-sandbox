<script lang="ts">
  let { status }: { status: string } = $props()

  const LIVE = ['ready', 'provisioning', 'stopped', 'starting']
  const klass = $derived(LIVE.includes(status) ? status : 'dead')
</script>

<span class="pill {klass}">{status}</span>

<style>
  .pill {
    display: inline-block;
    font-size: 0.75rem;
    font-weight: 500;
    padding: 0.1rem 0.5rem;
    border-radius: var(--radius);
    color: var(--dim);
    background: var(--hover);
    white-space: nowrap;
  }
  .ready {
    color: var(--green);
    background: color-mix(in srgb, var(--green) 12%, transparent);
  }
  .provisioning,
  .starting {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .dead {
    color: var(--red);
    background: color-mix(in srgb, var(--red) 10%, transparent);
  }
</style>

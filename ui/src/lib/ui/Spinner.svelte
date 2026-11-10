<script lang="ts">
  let { size = '0.9rem' }: { size?: string } = $props()
</script>

<span class="spin" style={`width:${size};height:${size}`} aria-hidden="true"></span>

<style>
  .spin {
    display: inline-block;
    flex-shrink: 0;
    border: 2px solid var(--line-strong);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: rotate 800ms linear infinite;
    vertical-align: -0.15em;
  }
  @keyframes rotate {
    to {
      transform: rotate(360deg);
    }
  }
</style>

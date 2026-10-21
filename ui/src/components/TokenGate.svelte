<script lang="ts">
  import { saveToken, api, RpcError, UNAUTHENTICATED } from '../lib/rpc'

  let { onAuth }: { onAuth(): void } = $props()

  let value = $state('')
  let error = $state('')
  let checking = $state(false)

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    const token = value.trim()
    if (!token || checking) return
    checking = true
    error = ''
    saveToken(token)
    try {
      // Validate before unlocking: a rejected token never reaches the
      // dashboard shell.
      await api.list()
      onAuth()
    } catch (e) {
      if (e instanceof RpcError && e.code === UNAUTHENTICATED) {
        error = 'token rejected by the api'
      } else {
        error = e instanceof Error ? e.message : 'request failed'
      }
    } finally {
      checking = false
    }
  }
</script>

<div class="gate">
  <form onsubmit={submit} class="panel">
    <p class="wordmark">signet sandbox</p>
    <h1 class="title">access</h1>
    <p class="hint">
      paste an api token. in local mode the profile token lives in
      <code>~/.signet/config.toml</code>.
    </p>
    <input class="input" type="password" placeholder="sgn_..." bind:value autocomplete="off" spellcheck="false" />
    {#if error}
      <p class="error-text">{error}</p>
    {/if}
    <button class="btn btn--primary" type="submit" disabled={checking}>
      {checking ? 'checking…' : 'unlock'}
    </button>
  </form>
</div>

<style>
  .gate {
    min-height: 100vh;
    display: grid;
    place-items: center;
  }
  form {
    width: min(26rem, 90vw);
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    animation: rise 400ms ease both;
  }
  .wordmark {
    color: var(--dim);
    font-size: 0.65rem;
    letter-spacing: 0.42em;
    text-transform: uppercase;
    margin: 0;
  }
  .title {
    font-size: 2rem;
    line-height: 1;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .hint {
    color: var(--dim);
    font-size: 0.75rem;
    line-height: 1.5;
    margin: 0;
  }
  @keyframes rise {
    from { transform: translateY(12px); opacity: 0; }
    to { transform: none; opacity: 1; }
  }
</style>

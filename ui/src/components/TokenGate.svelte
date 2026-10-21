<script lang="ts">
  import { saveToken, tokenFromFragment, storedToken, api, clearToken, RpcError, UNAUTHENTICATED } from '../lib/rpc'

  let value = $state(tokenFromFragment() ?? '')
  let error = $state('')

  if (tokenFromFragment()) {
    saveToken(tokenFromFragment()!)
    // Drop the secret from the address bar once it is stored.
    history.replaceState(null, '', location.pathname + location.search)
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    const token = value.trim()
    if (!token) return
    saveToken(token)
    try {
      await api.list()
      error = ''
    } catch (e) {
      if (e instanceof RpcError && e.code === UNAUTHENTICATED) {
        clearToken()
        error = 'token rejected by the api'
      } else {
        error = e instanceof Error ? e.message : 'request failed'
      }
    }
  }
</script>

<div class="gate">
  <form onsubmit={submit}>
    <p class="mark">signet</p>
    <h1>dashboard</h1>
    <p class="hint">
      paste an api token. in local mode the profile token lives in
      <code>~/.signet/config.toml</code>.
    </p>
    <input
      type="password"
      placeholder="sgn_..."
      bind:value
      autocomplete="off"
      spellcheck="false"
    />
    {#if error}
      <p class="error">{error}</p>
    {/if}
    <button type="submit">unlock</button>
    {#if storedToken()}
      <button type="button" class="ghost" onclick={() => { clearToken(); value = '' }}>
        forget stored token
      </button>
    {/if}
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
    border: 1px solid var(--line);
    background: var(--panel);
    padding: 2rem;
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    animation: rise 400ms ease both;
  }
  .mark {
    color: var(--amber);
    font-size: 0.65rem;
    letter-spacing: 0.4em;
    text-transform: uppercase;
  }
  h1 {
    font-size: 1.4rem;
    font-weight: 600;
    letter-spacing: 0.06em;
  }
  .hint {
    color: var(--dim);
    font-size: 0.75rem;
    line-height: 1.5;
  }
  code {
    color: var(--ink);
  }
  input {
    background: var(--bg);
    border: 1px solid var(--line);
    color: var(--ink);
    font: inherit;
    padding: 0.6rem 0.75rem;
    outline: none;
  }
  input:focus {
    border-color: var(--amber-dim);
  }
  button {
    font: inherit;
    background: var(--amber);
    color: #141005;
    border: none;
    padding: 0.6rem;
    cursor: pointer;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    font-size: 0.75rem;
  }
  button:hover {
    background: var(--amber-bright);
  }
  button.ghost {
    background: none;
    border: 1px solid var(--line);
    color: var(--dim);
  }
  button.ghost:hover {
    color: var(--ink);
    border-color: var(--dim);
  }
  .error {
    color: var(--red);
    font-size: 0.75rem;
  }
  @keyframes rise {
    from { transform: translateY(12px); opacity: 0; }
    to { transform: none; opacity: 1; }
  }
</style>

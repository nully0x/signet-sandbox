<script lang="ts" generics="T extends string | number | null | undefined">
  let { value = $bindable(), ...rest }: { value?: T } & Record<string, unknown> = $props()
</script>

<input {...rest} class="input" bind:value />

<style>
  .input {
    font: inherit;
    font-size: 0.88rem;
    background: var(--panel);
    border: 1px solid var(--line-strong);
    color: var(--ink);
    padding: 0.5rem 0.65rem;
    border-radius: var(--radius);
    outline: none;
    width: 100%;
  }
  .input:focus {
    border-color: var(--accent);
  }
  .input::placeholder {
    color: var(--faint);
  }
</style>

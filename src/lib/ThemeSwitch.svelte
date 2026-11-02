<script lang="ts">
  import { applyTheme, savedTheme, type Theme } from "$lib/theme";

  const options: { value: Theme; label: string }[] = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];

  let theme = $state(savedTheme());

  function choose(value: Theme) {
    theme = value;
    applyTheme(value);
  }
</script>

<div class="switch" role="radiogroup" aria-label="Appearance">
  {#each options as option (option.value)}
    <button
      role="radio"
      aria-checked={theme === option.value}
      class:active={theme === option.value}
      onclick={() => choose(option.value)}
    >
      {option.label}
    </button>
  {/each}
</div>

<style>
  .switch {
    display: inline-flex;
    padding: 2px;
    gap: 2px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  button {
    padding: 3px 10px;
    font-size: 12px;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--muted);
  }

  button.active {
    background: var(--panel-raised);
    color: var(--text);
    box-shadow: 0 1px 2px #0003;
  }
</style>

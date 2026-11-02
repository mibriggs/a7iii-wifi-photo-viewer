<script lang="ts">
  import ThemeSwitch from "$lib/ThemeSwitch.svelte";
  import { connect, type Session } from "$lib/camera";

  let { onConnected }: { onConnected: (session: Session) => void } = $props();

  const ADDRESS_KEY = "cameraAddress";

  let busy = $state(false);
  let found = $state<number | null>(null);
  let error = $state("");
  // Blank means "search the network". Set it to the fake camera's address for testing.
  let address = $state(readSaved());

  function readSaved(): string {
    try {
      return localStorage.getItem(ADDRESS_KEY) ?? "";
    } catch {
      return "";
    }
  }

  async function start() {
    busy = true;
    found = null;
    error = "";
    try {
      localStorage.setItem(ADDRESS_KEY, address.trim());
    } catch {}
    try {
      onConnected(await connect(address, (n) => (found = n)));
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="theme">
  <ThemeSwitch />
</div>

<main>
  <h1>Connect your camera</h1>

  <ol>
    <li>
      On the camera, press <b>MENU</b> and go to <b>Network › Send to Smartphone Func. › Send to Smartphone ›
      Select on Smartphone</b>. It shows a Wi-Fi name (<code>DIRECT-xxxx</code>) and a password.
    </li>
    <li>Join that Wi-Fi network on this computer. You'll be off the internet while connected to it.</li>
    <li>Click <b>Connect</b>.</li>
  </ol>

  <button class="primary connect" onclick={start} disabled={busy}>
    {busy ? "Connecting…" : "Connect"}
  </button>

  <p class="status" aria-live="polite">
    {#if busy && found === null}
      Looking for the camera…
    {:else if busy}
      Reading the card… {found} photos found
    {/if}
  </p>

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  <details>
    <summary>Advanced</summary>
    <label>
      Camera address
      <input
        bind:value={address}
        placeholder="Leave blank to search (e.g. http://127.0.0.1:64399/dd.xml for the fake camera)"
        spellcheck="false"
      />
    </label>
  </details>
</main>

<style>
  .theme {
    position: fixed;
    top: 12px;
    right: 16px;
  }

  /* Fills the window and centers its contents both ways. */
  main {
    display: grid;
    align-content: center;
    min-height: 100vh;
    max-width: 560px;
    margin: 0 auto;
    padding: 24px;
    text-align: center;
  }

  h1 {
    font-size: 22px;
    font-weight: 600;
    margin: 0 0 16px;
  }

  /* Numbers sit inside the text so they stay with each centered line. */
  ol {
    margin: 0 0 24px;
    padding: 0;
    list-style-position: inside;
    color: var(--muted);
  }

  li {
    margin-bottom: 8px;
  }

  b,
  code {
    color: var(--text);
  }

  .connect {
    justify-self: center;
    padding: 8px 28px;
    font-size: 15px;
  }

  .status {
    min-height: 1.45em;
    color: var(--muted);
  }

  .error {
    color: var(--error);
    background: color-mix(in srgb, var(--error) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--error) 35%, transparent);
    border-radius: var(--radius);
    padding: 10px 12px;
  }

  details {
    margin-top: 32px;
    color: var(--muted);
  }

  summary {
    cursor: pointer;
  }

  label {
    display: grid;
    gap: 6px;
    margin-top: 10px;
  }

  input {
    text-align: left;
  }
</style>

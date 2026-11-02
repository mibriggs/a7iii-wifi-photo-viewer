<script lang="ts">
  import ConnectScreen from "$lib/ConnectScreen.svelte";
  import Gallery from "$lib/Gallery.svelte";
  import { disconnect, type Session } from "$lib/camera";

  let session = $state<Session | null>(null);

  async function leave() {
    await disconnect();
    session = null;
  }
</script>

{#if session}
  <Gallery {session} onDisconnect={leave} />
{:else}
  <ConnectScreen onConnected={(s) => (session = s)} />
{/if}

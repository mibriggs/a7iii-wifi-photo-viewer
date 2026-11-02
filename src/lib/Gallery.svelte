<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { SvelteSet } from "svelte/reactivity";
  import ProgressBar from "$lib/ProgressBar.svelte";
  import ThemeSwitch from "$lib/ThemeSwitch.svelte";
  import Viewer from "$lib/Viewer.svelte";
  import {
    checkSaved,
    download,
    downloadLabel,
    formatDay,
    formatMB,
    formatTime,
    fractionDone,
    imageUrl,
    setDownloadDir,
    watchDownloads,
    type DownloadProgress,
    type DownloadStatus,
    type Folder,
    type Photo,
    type Session,
  } from "$lib/camera";

  let { session, onDisconnect }: { session: Session; onDisconnect: () => void } = $props();

  // Newest first. The camera lists date folders oldest first, and photos in shooting order.
  const ordered = $derived([...session.photos].reverse());
  const positionOf = $derived(new Map(ordered.map((p, i) => [p.index, i])));
  const groups = $derived.by(() => {
    const result: { key: string; photos: Photo[] }[] = [];
    for (const photo of ordered) {
      const key = photo.folder || photo.date.slice(0, 10);
      if (result.at(-1)?.key !== key) result.push({ key, photos: [] });
      result.at(-1)!.photos.push(photo);
    }
    return result;
  });

  const selected = new SvelteSet<number>();
  let status = $state<Record<number, DownloadStatus>>({});
  let errors = $state<Record<number, string>>({});
  let progress = $state<Record<number, DownloadProgress>>({});
  let lastSaved = $state("");
  let viewing = $state<number | null>(null);
  let downloadDir = $state("");
  let folderError = $state("");
  // "Documents › SonyCamera": the parent too, so two folders with the same name look different.
  const folderLabel = $derived(downloadDir.split(/[\\/]/).filter(Boolean).slice(-2).join(" › "));
  const anySaved = $derived(Object.values(status).includes("saved"));
  // When each download finished (ms), so a folder check that started earlier can't undo it.
  const finishedAt: Record<number, number> = {};

  // Files may have been added or deleted in Finder while the app was in the background.
  $effect(() => {
    const stop = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) return;
      const started = Date.now();
      checkSaved()
        .then((folder) => applyFolder(folder, started))
        .catch(() => {});
    });
    return () => void stop.then((unlisten) => unlisten());
  });

  async function chooseFolder() {
    const picked = await open({ directory: true, defaultPath: downloadDir, title: "Choose where to save photos" });
    if (typeof picked !== "string") return;
    try {
      applyFolder(await setDownloadDir(picked));
      lastSaved = "";
      folderError = "";
    } catch (e) {
      folderError = String(e);
    }
  }

  /** Marks the photos that have a file in the download folder as Saved, leaving downloads that
   *  are queued or running alone. `checkedAt` is when the check began, if it was a re-check. */
  function applyFolder(folder: Folder, checkedAt = Infinity) {
    downloadDir = folder.downloadDir;
    const saved = new Set(folder.saved);
    for (const photo of session.photos) {
      const current = status[photo.index];
      if (current === "queued" || current === "downloading") continue;
      if (saved.has(photo.index)) status[photo.index] = "saved";
      else if (current === "saved" && !((finishedAt[photo.index] ?? 0) > checkedAt)) delete status[photo.index];
    }
  }

  // The gallery is created afresh for each connection, so the starting value is the one we want.
  // svelte-ignore state_referenced_locally
  applyFolder(session);

  $effect(() => {
    const stop = watchDownloads((p) => (progress[p.index] = p));
    return () => void stop.then((unlisten) => unlisten());
  });

  const pending = $derived(Object.values(status).filter((s) => s === "queued" || s === "downloading").length);
  const failed = $derived(Object.values(status).filter((s) => s === "error").length);

  // The photos in the current run of downloads, for the header's overall bar.
  // Starts afresh when a download is started after the previous run finished.
  let batch = $state<number[]>([]);
  const batchDone = $derived(batch.filter((i) => status[i] === "saved" || status[i] === "error").length);
  const batchBytes = $derived(batch.reduce((sum, i) => sum + (progress[i]?.received ?? 0), 0));
  const batchFraction = $derived.by(() => {
    if (batch.length === 0) return 0;
    const current = batch.find((i) => status[i] === "downloading");
    const partial = current === undefined ? 0 : (fractionDone(progress[current]) ?? 0);
    return (batchDone + partial) / batch.length;
  });

  // One download at a time: the camera's Wi-Fi is slow and its server is small.
  const queue: number[] = [];
  let running = false;

  function enqueue(indexes: number[]) {
    if (!running) batch = [];
    for (const index of indexes) {
      if (status[index] === undefined || status[index] === "error") {
        status[index] = "queued";
        queue.push(index);
        batch.push(index);
      }
    }
    if (!running) void runQueue();
  }

  async function runQueue() {
    running = true;
    while (queue.length > 0) {
      const index = queue.shift()!;
      status[index] = "downloading";
      try {
        lastSaved = (await download(index)).path;
        status[index] = "saved";
        finishedAt[index] = Date.now();
        delete errors[index];
      } catch (e) {
        status[index] = "error";
        errors[index] = String(e);
      }
    }
    running = false;
  }

  function downloadSelected() {
    enqueue(ordered.filter((p) => selected.has(p.index)).map((p) => p.index));
    selected.clear();
  }

  function toggle(index: number) {
    if (selected.has(index)) selected.delete(index);
    else selected.add(index);
  }

  function toggleGroup(photos: Photo[]) {
    const allSelected = photos.every((p) => selected.has(p.index));
    for (const p of photos) {
      if (allSelected) selected.delete(p.index);
      else selected.add(p.index);
    }
  }
</script>

<header>
  <div class="title">
    <h1>{session.cameraName}</h1>
    <span class="muted">{session.photos.length} photos</span>
    <span class="muted">
      · Saving to
      <button class="link" onclick={chooseFolder} title="{downloadDir} (click to change)">{folderLabel}</button>
    </span>
    {#if folderError}
      <span class="failed">{folderError}</span>
    {/if}
  </div>

  <div class="actions">
    {#if pending > 0}
      <div class="batch">
        <span class="muted" aria-live="polite">
          Downloading {Math.min(batchDone + 1, batch.length)} of {batch.length} · {formatMB(batchBytes)}
        </span>
        <ProgressBar value={batchFraction} label="All downloads" />
      </div>
    {:else if failed > 0}
      <span class="failed">{failed} failed</span>
    {/if}
    {#if selected.size > 0}
      <button onclick={() => selected.clear()}>Clear selection</button>
      <button class="primary" onclick={downloadSelected}>Download {selected.size} selected</button>
    {:else}
      <button onclick={() => ordered.forEach((p) => selected.add(p.index))}>Select all</button>
    {/if}
    <button onclick={() => revealItemInDir(lastSaved || downloadDir)} disabled={!lastSaved && !anySaved} title={downloadDir}>
      Show in Finder
    </button>
    <button onclick={onDisconnect}>Disconnect</button>
    <ThemeSwitch />
  </div>
</header>

<main class:selecting={selected.size > 0}>
  {#if ordered.length === 0}
    <p class="muted empty">No photos on the card. (Videos aren't shown.)</p>
  {/if}

  {#each groups as group (group.key)}
    <section>
      <div class="day">
        <h2>{formatDay(group.key)}</h2>
        <button class="link" onclick={() => toggleGroup(group.photos)}>
          {group.photos.every((p) => selected.has(p.index)) ? "Deselect day" : "Select day"}
        </button>
      </div>

      <div class="grid">
        {#each group.photos as photo (photo.index)}
          <figure class:selected={selected.has(photo.index)}>
            <div class="media">
              <button class="thumb" onclick={() => (viewing = positionOf.get(photo.index) ?? null)}>
                <img src={imageUrl(session.sessionId, "tn", photo.index)} loading="lazy" alt={photo.title} />
              </button>
              {#if status[photo.index] === "downloading"}
                <div class="tile-progress">
                  <ProgressBar value={fractionDone(progress[photo.index])} label="Downloading {photo.title}" />
                </div>
              {/if}
            </div>
            <input
              class="check"
              type="checkbox"
              checked={selected.has(photo.index)}
              onchange={() => toggle(photo.index)}
              aria-label="Select {photo.title}"
            />
            {#if status[photo.index]}
              <span class="badge {status[photo.index]}" title={errors[photo.index] ?? ""}>
                {downloadLabel(status[photo.index], progress[photo.index])}
              </span>
            {/if}
            <figcaption>
              <span>{photo.title}</span>
              <span class="muted">{formatTime(photo.date)}</span>
            </figcaption>
          </figure>
        {/each}
      </div>
    </section>
  {/each}
</main>

{#if viewing !== null}
  <Viewer
    sessionId={session.sessionId}
    photos={ordered}
    position={viewing}
    onMove={(position) => (viewing = position)}
    {status}
    {errors}
    {progress}
    onDownload={(index) => enqueue([index])}
    onClose={() => (viewing = null)}
  />
{/if}

<style>
  header {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
    padding: 12px 20px;
    background: color-mix(in srgb, var(--panel) 92%, transparent);
    backdrop-filter: blur(12px);
    border-bottom: 1px solid var(--border);
  }

  .title {
    display: flex;
    gap: 10px;
    align-items: baseline;
  }

  h1 {
    margin: 0;
    font-size: 17px;
    font-weight: 600;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }

  .muted {
    color: var(--muted);
  }

  .failed {
    color: var(--error);
  }

  .batch {
    display: grid;
    gap: 4px;
    min-width: 220px;
  }

  .media {
    position: relative;
  }

  /* Sits along the bottom edge of the thumbnail. */
  .tile-progress {
    position: absolute;
    left: 6px;
    right: 6px;
    bottom: 6px;
  }

  main {
    padding: 8px 20px 40px;
  }

  .empty {
    margin-top: 20vh;
    text-align: center;
  }

  .day {
    display: flex;
    gap: 12px;
    align-items: baseline;
    margin: 20px 0 10px;
  }

  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
  }

  /* 6 across on a full-screen 16" MacBook Pro at its default scaling (1728 wide). */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: 20px;
  }

  figure {
    position: relative;
    margin: 0;
    background: var(--panel);
    border: 2px solid transparent;
    border-radius: var(--radius);
    overflow: hidden;
  }

  figure.selected {
    border-color: var(--accent);
  }

  .thumb {
    display: block;
    width: 100%;
    padding: 0;
    border: none;
    border-radius: 0;
    background: var(--panel-raised);
    cursor: zoom-in;
  }

  .thumb img {
    display: block;
    width: 100%;
    aspect-ratio: 3 / 2;
    object-fit: cover;
  }

  /* Checkboxes appear on hover, and stay visible once anything is selected. */
  .check {
    position: absolute;
    top: 8px;
    left: 8px;
    width: 18px;
    height: 18px;
    margin: 0;
    accent-color: var(--accent);
    opacity: 0;
    cursor: pointer;
  }

  figure:hover .check,
  .check:focus-visible,
  .selecting .check {
    opacity: 1;
  }

  /* Badges sit on top of photos, so they keep the same colours in both themes. */
  .badge {
    position: absolute;
    top: 8px;
    right: 8px;
    padding: 1px 7px;
    border-radius: 999px;
    font-size: 12px;
    color: #fff;
    background: #000a;
  }

  .badge.saved {
    background: #34c759;
    color: #000;
  }

  .badge.error {
    background: #ff6961;
    color: #000;
  }

  figcaption {
    display: flex;
    justify-content: space-between;
    gap: 6px;
    padding: 6px 8px;
    font-size: 12px;
    white-space: nowrap;
  }

  figcaption span:first-child {
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>

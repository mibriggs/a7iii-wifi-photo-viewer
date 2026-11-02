<script lang="ts">
  import ProgressBar from "$lib/ProgressBar.svelte";
  import ZoomableImage from "$lib/ZoomableImage.svelte";
  import {
    downloadLabel,
    formatDay,
    formatTime,
    fractionDone,
    imageUrl,
    type DownloadProgress,
    type DownloadStatus,
    type Photo,
  } from "$lib/camera";

  let {
    sessionId,
    photos,
    position,
    status,
    errors,
    progress,
    onMove,
    onDownload,
    onClose,
  }: {
    sessionId: number;
    photos: Photo[];
    position: number;
    status: Record<number, DownloadStatus>;
    errors: Record<number, string>;
    progress: Record<number, DownloadProgress>;
    onMove: (position: number) => void;
    onDownload: (index: number) => void;
    onClose: () => void;
  } = $props();

  const photo = $derived(photos[position]);
  const photoStatus = $derived(status[photo.index]);

  let image = $state<ReturnType<typeof ZoomableImage>>();
  let zoomPercent = $state(0);
  let zoomed = $state(false);
  let original = $state<"not needed" | "loading" | "loaded" | "failed">("not needed");
  let originalPercent = $state<number | null>(null);
  let previewReady = $state(false);

  // Once this photo's preview is showing, fetch the previous and next ones in the background so
  // the arrow keys feel instant. (The app keeps fetched images in memory while connected.)
  $effect(() => {
    if (!previewReady) return;
    for (const neighbour of [photos[position - 1], photos[position + 1]]) {
      if (neighbour) new Image().src = imageUrl(sessionId, "lrg", neighbour.index);
    }
  });

  function move(step: number) {
    const next = position + step;
    if (next >= 0 && next < photos.length) onMove(next);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") onClose();
    else if (e.key === "ArrowLeft") move(-1);
    else if (e.key === "ArrowRight") move(1);
    else if (e.key === "+" || e.key === "=") image?.zoomBy(1.25);
    else if (e.key === "-") image?.zoomBy(0.8);
    else if (e.key === "0") image?.fitToScreen();
    else return;
    e.preventDefault();
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- The photo always sits on black, in either theme, so it's judged against a neutral background;
     the panel below follows the Light/Dark setting. -->
<div class="viewer" role="dialog" aria-modal="true" aria-label={photo.title}>
  <!-- A fresh image (at fit-to-screen) for each photo. Clicking the dark area around it closes the viewer. -->
  {#key photo.index}
    <ZoomableImage
      bind:this={image}
      bind:percent={zoomPercent}
      bind:zoomed
      bind:original
      bind:originalPercent
      bind:previewReady
      {sessionId}
      index={photo.index}
      alt={photo.title}
      onBackgroundClick={onClose}
    />
  {/key}

  <!-- These sit on the black photo area, so they use the dark colours in either theme. -->
  <button class="close always-dark" onclick={onClose} aria-label="Close" title="Close (Esc)">×</button>
  <button class="nav prev always-dark" onclick={() => move(-1)} disabled={position === 0} aria-label="Previous photo">
    ‹
  </button>
  <button
    class="nav next always-dark"
    onclick={() => move(1)}
    disabled={position === photos.length - 1}
    aria-label="Next photo"
  >
    ›
  </button>

  <footer>
    {#if photoStatus === "downloading"}
      <div class="footer-progress">
        <ProgressBar value={fractionDone(progress[photo.index])} label="Downloading {photo.title}" />
      </div>
    {/if}
    <div class="info">
      <strong>{photo.title}</strong>
      <span class="muted">
        {formatDay(photo.date || photo.folder)}
        {formatTime(photo.date)} · {position + 1} of {photos.length}
      </span>
      {#if photoStatus === "error"}
        <span class="error">{errors[photo.index]}</span>
      {/if}
    </div>
    <div class="actions">
      {#if original === "loading"}
        <span class="muted">
          Loading full resolution…{originalPercent === null ? "" : ` ${originalPercent}%`}
        </span>
      {:else if original === "failed"}
        <span class="error">Full resolution unavailable</span>
      {/if}
      <div class="zoom-controls" role="group" aria-label="Zoom">
        <button onclick={() => image?.zoomBy(0.8)} disabled={!zoomed} aria-label="Zoom out" title="Zoom out (−)">
          −
        </button>
        <span class="zoom" title="100% = one camera pixel per screen pixel">{zoomPercent}%</span>
        <button onclick={() => image?.zoomBy(1.25)} aria-label="Zoom in" title="Zoom in (+)">+</button>
        <button onclick={() => image?.fitToScreen()} disabled={!zoomed} title="Fit to screen (0)">Fit</button>
      </div>
      <button
        class:primary={photoStatus === undefined || photoStatus === "error"}
        onclick={() => onDownload(photo.index)}
        disabled={photoStatus !== undefined && photoStatus !== "error"}
      >
        {downloadLabel(photoStatus, progress[photo.index])}
      </button>
      <button onclick={onClose}>Close</button>
    </div>
  </footer>
</div>

<style>
  .viewer {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: grid;
    grid-template-rows: 1fr auto;
    background: #000;
    -webkit-user-select: none;
    user-select: none;
  }

  .nav {
    position: absolute;
    top: calc(50% - 40px);
    width: 44px;
    height: 80px;
    padding: 0;
    font-size: 32px;
    line-height: 1;
    background: #ffffff14;
    border: none;
  }

  .nav.prev {
    left: 10px;
  }

  .nav.next {
    right: 10px;
  }

  .nav:disabled {
    opacity: 0.15;
  }

  .close {
    position: absolute;
    top: 12px;
    right: 12px;
    width: 36px;
    height: 36px;
    padding: 0;
    font-size: 22px;
    line-height: 1;
    background: #ffffff14;
    border: none;
    border-radius: 50%;
  }

  .close:hover {
    background: #ffffff2e;
  }

  footer {
    position: relative;
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
    padding: 12px 20px;
    background: var(--panel);
    border-top: 1px solid var(--border);
  }

  /* Runs along the top edge of the footer. */
  .footer-progress {
    position: absolute;
    top: -2px;
    left: 0;
    right: 0;
  }

  .info {
    display: grid;
    gap: 2px;
  }

  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .zoom-controls {
    display: flex;
    gap: 4px;
    align-items: center;
    margin-right: 8px;
  }

  .zoom-controls button {
    min-width: 32px;
    padding: 6px 8px;
  }

  .zoom {
    min-width: 5ch;
    text-align: center;
    font-variant-numeric: tabular-nums;
    color: var(--muted);
  }

  .muted {
    color: var(--muted);
  }

  .error {
    color: var(--error);
  }
</style>

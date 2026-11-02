<script lang="ts">
  import { fractionDone, imageUrl, watchOriginals } from "$lib/camera";

  let {
    sessionId,
    index,
    alt,
    percent = $bindable(0),
    zoomed = $bindable(false),
    original = $bindable("not needed"),
    originalPercent = $bindable(null),
    previewReady = $bindable(false),
    onBackgroundClick,
  }: {
    sessionId: number;
    index: number;
    alt: string;
    /** Zoom level relative to the original's pixels: 100 = one camera pixel per screen pixel. */
    percent?: number;
    zoomed?: boolean;
    /** The full-size original is fetched the first time the photo is zoomed in. */
    original?: "not needed" | "loading" | "loaded" | "failed";
    /** How much of the original has arrived, 0–100; null if the camera didn't send its size. */
    originalPercent?: number | null;
    /** True once the preview has loaded. */
    previewReady?: boolean;
    onBackgroundClick: () => void;
  } = $props();

  // Each photo gets a new one of these; the viewer's bound values may still describe the last photo.
  previewReady = false;
  originalPercent = null;

  $effect(() => {
    const stop = watchOriginals((p) => {
      if (p.index !== index) return;
      const fraction = fractionDone(p);
      originalPercent = fraction === null ? null : Math.round(fraction * 100);
    });
    return () => void stop.then((unlisten) => unlisten());
  });

  // Until the original has loaded we don't know its width; the A7 III's is 6000 px.
  const ASSUMED_FULL_WIDTH = 6000;
  const MAX_PERCENT = 400;
  // Space kept clear around the photo when it fits the screen (room for the arrow buttons).
  const MARGIN_X = 64;
  const MARGIN_Y = 16;

  let stage = $state<HTMLDivElement>();
  let stageWidth = $state(0);
  let stageHeight = $state(0);
  let aspect = $state(3 / 2);
  let previewLoaded = $state(false);
  let wantOriginal = $state(false);
  let originalWidth = $state<number | null>(null);
  let originalFailed = $state(false);

  // The photo's displayed size as a multiple of its fit-to-screen size, and its top-left corner
  // in the stage as last set. `position` keeps that within bounds.
  let scale = $state(1);
  let rawX = $state(0);
  let rawY = $state(0);

  const fit = $derived.by(() => {
    const w = Math.max(stageWidth - 2 * MARGIN_X, 1);
    const h = Math.max(stageHeight - 2 * MARGIN_Y, 1);
    return w / h > aspect ? { w: h * aspect, h } : { w, h: w / aspect };
  });
  const width = $derived(fit.w * scale);
  const height = $derived(fit.h * scale);
  const fullWidth = $derived(originalWidth ?? ASSUMED_FULL_WIDTH);
  // Scale at which one camera pixel covers one screen pixel.
  const actualSizeScale = $derived(fullWidth / devicePixelRatio / fit.w);
  const maxScale = $derived(Math.max(1, (actualSizeScale * MAX_PERCENT) / 100));
  const position = $derived(clampPosition(rawX, rawY, width, height));

  $effect(() => {
    percent = Math.round(((width * devicePixelRatio) / fullWidth) * 100);
    zoomed = scale > 1.001;
    original = !wantOriginal
      ? "not needed"
      : originalFailed
        ? "failed"
        : originalWidth === null
          ? "loading"
          : "loaded";
  });

  /** Centres the photo along any side where it's smaller than the stage; otherwise keeps it
   *  from being dragged past its edges. */
  function clampPosition(x: number, y: number, w: number, h: number) {
    return {
      x: w <= stageWidth ? (stageWidth - w) / 2 : Math.min(0, Math.max(stageWidth - w, x)),
      y: h <= stageHeight ? (stageHeight - h) / 2 : Math.min(0, Math.max(stageHeight - h, y)),
    };
  }

  /** Zooms to `newScale`, keeping the point under (px, py) in the stage where it is. */
  function zoomTo(newScale: number, px = stageWidth / 2, py = stageHeight / 2) {
    newScale = Math.min(maxScale, Math.max(1, newScale));
    const u = (px - position.x) / width;
    const v = (py - position.y) / height;
    scale = newScale;
    rawX = px - u * fit.w * newScale;
    rawY = py - v * fit.h * newScale;
    if (newScale > 1) wantOriginal = true;
  }

  export function zoomBy(factor: number) {
    zoomTo(scale * factor);
  }

  export function fitToScreen() {
    scale = 1;
  }

  function stagePoint(e: { clientX: number; clientY: number }): [number, number] {
    const rect = stage!.getBoundingClientRect();
    return [e.clientX - rect.left, e.clientY - rect.top];
  }

  function onDoubleClick(e: MouseEvent) {
    if (zoomed) fitToScreen();
    else zoomTo(actualSizeScale, ...stagePoint(e));
  }

  // Dragging to move around while zoomed in.
  let drag = $state<{ x: number; y: number; startX: number; startY: number } | null>(null);

  function onPointerDown(e: PointerEvent) {
    if (!zoomed || e.button !== 0) return;
    drag = { x: e.clientX, y: e.clientY, startX: position.x, startY: position.y };
    stage!.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!drag) return;
    rawX = drag.startX + e.clientX - drag.x;
    rawY = drag.startY + e.clientY - drag.y;
  }

  function onPointerUp() {
    drag = null;
  }

  function onPreviewLoad(e: Event) {
    const img = e.currentTarget as HTMLImageElement;
    previewLoaded = true;
    previewReady = true;
    aspect = img.naturalWidth / img.naturalHeight;
  }

  function onClick(e: MouseEvent) {
    if (!zoomed && e.target === stage) onBackgroundClick();
  }

  // Scroll wheel and trackpad pinch. Pinch arrives either as wheel events with ctrlKey set or,
  // in Safari's engine (which Tauri uses on macOS), as gesture events; whichever comes is used.
  // These are added by hand because they must be able to cancel the page's own zooming.
  $effect(() => {
    const el = stage!;
    let gesturing = false;
    let gestureStartScale = 1;

    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (gesturing) return;
      const delta = e.deltaMode === WheelEvent.DOM_DELTA_LINE ? e.deltaY * 16 : e.deltaY;
      zoomTo(scale * Math.exp(-delta * (e.ctrlKey ? 0.01 : 0.002)), ...stagePoint(e));
    };
    type GestureEvent = UIEvent & { scale: number; clientX: number; clientY: number };
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      gesturing = true;
      gestureStartScale = scale;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const g = e as GestureEvent;
      zoomTo(gestureStartScale * g.scale, ...stagePoint(g));
    };
    const onGestureEnd = (e: Event) => {
      e.preventDefault();
      gesturing = false;
    };

    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("gesturestart", onGestureStart);
    el.addEventListener("gesturechange", onGestureChange);
    el.addEventListener("gestureend", onGestureEnd);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", onGestureStart);
      el.removeEventListener("gesturechange", onGestureChange);
      el.removeEventListener("gestureend", onGestureEnd);
    };
  });
</script>

<!-- Keyboard zoom (+, -, 0) is handled by the viewer around this. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="stage"
  class:zoomed
  class:dragging={drag !== null}
  bind:this={stage}
  bind:clientWidth={stageWidth}
  bind:clientHeight={stageHeight}
  onclick={onClick}
  onmousedown={(e) => e.detail > 1 && e.preventDefault()}
  ondblclick={onDoubleClick}
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
>
  <div
    class="frame"
    style:left="{position.x}px"
    style:top="{position.y}px"
    style:width="{width}px"
    style:height="{height}px"
  >
    <!-- Thumbnail (blurred) until the preview loads; the original replaces both once zoomed in. -->
    <img class="placeholder" src={imageUrl(sessionId, "tn", index)} alt="" draggable="false" />
    <img
      class:shown={previewLoaded}
      src={imageUrl(sessionId, "lrg", index)}
      {alt}
      draggable="false"
      onload={onPreviewLoad}
    />
    {#if wantOriginal}
      <img
        class:shown={originalWidth !== null}
        src={imageUrl(sessionId, "org", index)}
        alt=""
        draggable="false"
        onload={(e) => (originalWidth = (e.currentTarget as HTMLImageElement).naturalWidth)}
        onerror={() => (originalFailed = true)}
      />
    {/if}
  </div>
</div>

<style>
  .stage {
    position: relative;
    min-height: 0;
    overflow: hidden;
    /* Stops double-clicks from highlighting the photo. Safari's engine needs the prefixed one. */
    -webkit-user-select: none;
    user-select: none;
  }

  .frame {
    position: absolute;
    cursor: zoom-in;
  }

  .zoomed,
  .zoomed .frame {
    cursor: grab;
  }

  .dragging,
  .dragging .frame {
    cursor: grabbing;
  }

  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    opacity: 0;
    transition: opacity 0.15s;
  }

  img.shown {
    opacity: 1;
  }

  .placeholder {
    opacity: 1;
    filter: blur(6px);
  }
</style>

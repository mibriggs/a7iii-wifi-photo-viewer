<script lang="ts">
  /** `value` is 0–1, or null when the total isn't known (shows a moving stripe instead). */
  let { value, label }: { value: number | null; label: string } = $props();
</script>

<div
  class="bar"
  class:unknown={value === null}
  role="progressbar"
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={value === null ? undefined : Math.round(value * 100)}
>
  <div class="fill" style:width={value === null ? "30%" : `${value * 100}%`}></div>
</div>

<style>
  .bar {
    height: 4px;
    overflow: hidden;
    background: color-mix(in srgb, var(--text) 15%, transparent);
    border-radius: 2px;
  }

  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.15s linear;
  }

  .unknown .fill {
    animation: slide 1.2s ease-in-out infinite;
  }

  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>

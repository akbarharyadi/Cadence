<script lang="ts">
  let { checked, onchange, label = "" }: { checked: boolean; onchange: (v: boolean) => void; label?: string } = $props();
</script>

<button class="toggle" class:on={checked} role="switch" aria-checked={checked} aria-label={label} onclick={() => onchange(!checked)}>
  <span class="knob"></span>
</button>

<style>
  .toggle {
    width: 42px;
    height: 24px;
    flex: none;
    border-radius: 12px;
    background: var(--line-strong);
    position: relative;
    transition: background 0.2s;
  }
  .toggle.on {
    background: var(--accent);
  }
  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--text);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
    transition: transform 0.2s;
  }
  .on .knob {
    transform: translateX(18px);
  }
  .toggle:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
</style>

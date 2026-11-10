<script lang="ts">
  import { type IconName, icons } from "./icons";

  let { name }: { name: IconName } = $props();
</script>

<svg
  class="icon"
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.5"
  stroke-linecap="square"
  stroke-linejoin="miter"
  aria-hidden="true"
  focusable="false">{@html icons[name]}</svg
>

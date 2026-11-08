<script lang="ts">
  import * as api from './api';
  import * as format from './format';

  let { onexpired }: { onexpired: () => void } = $props();

  let listing = $state<api.Listing | null>(null);
  let error = $state('');
  let showHidden = $state(false);
  let visible = $derived(listing ? listing.children.filter((n) => showHidden || !n.name.startsWith('.')) : []);

  async function load(folder: string | null) {
    error = '';
    try {
      listing = await api.list(folder);
    } catch (e) {
      const err = api.asError(e);
      if (err.needs_login) onexpired();
      else error = err.message;
    }
  }

  // Each folder opened is a history entry, so Android's Back button (which goes
  // back in the WebView's history) returns to the previous folder; at the first
  // one it leaves the app, as usual.
  function open(folder: string | null) {
    if (folder === (listing?.folder.parent_id === null ? null : listing?.folder.id)) return;
    history.pushState({ folder }, '');
    load(folder);
  }

  function back(event: PopStateEvent) {
    load(event.state?.folder ?? null);
  }

  $effect(() => {
    history.replaceState({ folder: null }, '');
    load(null);
  });
</script>

<svelte:window onpopstate={back} />

<header>
  <nav aria-label="Путь">
    <button class="crumb" onclick={() => open(null)}>Файлы</button>
    {#if listing}
      {#each listing.path.slice(1) as dir (dir.id)}
        <span class="muted">/</span>
        <button class="crumb" onclick={() => open(dir.id)}>{dir.name}</button>
      {/each}
      {#if listing.folder.parent_id}
        <span class="muted">/</span>
        <span class="current">{listing.folder.name}</span>
      {/if}
    {/if}
  </nav>
</header>

{#if error}
  <p class="error pad">{error} <button onclick={() => load(listing?.folder.id ?? null)}>Повторить</button></p>
{/if}

<main>
  {#if listing}
    {#if visible.length === 0}
      <p class="muted pad">Папка пуста</p>
    {:else}
      <ul>
        {#each visible as node (node.id)}
          <li>
            {#if node.is_dir}
              <button class="row" onclick={() => open(node.id)}>
                <span class="icon" aria-hidden="true">📁</span>
                <span class="name">{node.name}</span>
                <span class="muted size"></span>
                <span class="muted when">{format.date(node.updated_at)}</span>
              </button>
            {:else}
              <div class="row">
                <span class="icon" aria-hidden="true">📄</span>
                <span class="name">{node.name}</span>
                <span class="muted size">{format.size(node.size)}</span>
                <span class="muted when">{format.date(node.modified_at ?? node.updated_at)}</span>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</main>

<footer>
  <label><input type="checkbox" bind:checked={showHidden} /> Показывать скрытые</label>
</footer>

<style>
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  nav {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }

  .crumb {
    border: none;
    background: none;
    padding: 4px 6px;
  }

  .current {
    padding: 4px 6px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  main {
    padding: 8px 8px 48px;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .row {
    display: grid;
    grid-template-columns: 28px 1fr 90px 150px;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px;
    border: none;
    border-radius: 6px;
    background: none;
    text-align: left;
  }

  @media (hover: hover) {
    button.row:hover,
    div.row:hover {
      background: var(--hover);
    }
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .size {
    text-align: right;
  }

  @media (max-width: 560px) {
    .row {
      grid-template-columns: 28px 1fr 80px;
    }
    .when {
      display: none;
    }
  }

  .pad {
    padding: 8px 16px;
  }

  footer {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    padding: 8px 16px;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }

  footer input {
    width: auto;
  }
</style>

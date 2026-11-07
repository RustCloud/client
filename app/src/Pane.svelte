<script lang="ts" module>
  export interface Item {
    key: string;
    name: string;
    /** Shown greyed next to the name. */
    note?: string;
  }
  export interface Crumb {
    label: string;
    go: () => void;
  }
</script>

<script lang="ts">
  // One side of the pair form: a list of folders to walk through and pick from.
  let {
    title,
    icon,
    crumbs,
    here,
    items,
    selected,
    error,
    onselect,
    onopen,
    oncreate,
  }: {
    title: string;
    icon: string;
    crumbs: Crumb[];
    /** The folder shown, itself selectable; none in a list of places. */
    here: Item | null;
    items: Item[];
    selected: string | null;
    error: string;
    onselect: (item: Item) => void;
    onopen: (item: Item) => void;
    /** Makes a folder in `here`; none in a list of places. */
    oncreate: ((name: string) => Promise<void>) | null;
  } = $props();

  let naming = $state(false);
  let name = $state('');
  let busy = $state(false);

  async function create() {
    if (!oncreate || !name.trim()) return;
    busy = true;
    try {
      await oncreate(name.trim());
      naming = false;
      name = '';
    } finally {
      busy = false;
    }
  }
</script>

<section class="pane">
  <h3>{title}</h3>
  <nav>
    {#each crumbs as crumb, i (i)}
      {#if i > 0}<span class="muted">›</span>{/if}
      <button class="crumb" onclick={crumb.go}>{crumb.label}</button>
    {/each}
  </nav>
  <ul role="listbox" aria-label={title}>
    {#if here}
      <li>
        <button
          class="row here"
          class:selected={selected === here.key}
          role="option"
          aria-selected={selected === here.key}
          onclick={() => onselect(here)}
        >
          <span aria-hidden="true">{icon}</span>
          <span class="name">{here.name} <span class="muted">(эта папка)</span></span>
          {#if here.note}<span class="note">{here.note}</span>{/if}
        </button>
      </li>
    {/if}
    {#each items as item (item.key)}
      <li class="item">
        <button
          class="row"
          class:selected={selected === item.key}
          role="option"
          aria-selected={selected === item.key}
          onclick={() => onselect(item)}
          ondblclick={() => onopen(item)}
        >
          <span aria-hidden="true">📁</span>
          <span class="name">{item.name}</span>
          {#if item.note}<span class="note">{item.note}</span>{/if}
        </button>
        <button class="open" title="Открыть" aria-label="Открыть {item.name}" onclick={() => onopen(item)}>›</button>
      </li>
    {:else}
      <li class="muted none">Вложенных папок нет</li>
    {/each}
  </ul>
  {#if oncreate}
    {#if naming}
      <div class="new">
        <!-- svelte-ignore a11y_autofocus -->
        <input
          bind:value={name}
          placeholder="Имя папки"
          autofocus
          onkeydown={(e) => (e.key === 'Enter' ? create() : e.key === 'Escape' && (naming = false))}
        />
        <button onclick={create} disabled={!name.trim() || busy}>Создать</button>
        <button onclick={() => (naming = false)}>✕</button>
      </div>
    {:else}
      <button class="add" onclick={() => (naming = true)}>+ Новая папка</button>
    {/if}
  {/if}
  {#if error}<p class="error">{error}</p>{/if}
</section>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    padding: 10px;
  }

  h3 {
    margin: 0 0 4px;
    font-size: 14px;
  }

  nav {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px;
    margin-bottom: 6px;
    font-size: 13px;
  }

  .crumb {
    border: none;
    background: none;
    padding: 2px 4px;
  }

  ul {
    list-style: none;
    margin: 0 0 8px;
    padding: 0;
    height: 260px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .item {
    display: flex;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
    border: none;
    border-radius: 0;
    background: none;
    text-align: left;
    padding: 6px 8px;
  }

  .row.here {
    width: 100%;
    border-bottom: 1px dashed var(--border);
  }

  .row.selected,
  .row.selected:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-text);
  }

  .row.selected .muted,
  .row.selected .note {
    color: inherit;
  }

  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .note {
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }

  .open {
    border: none;
    border-radius: 0;
    background: none;
    padding: 0 12px;
    font-size: 16px;
  }

  .none {
    padding: 6px 8px;
  }

  .new {
    display: flex;
    gap: 6px;
  }

  .new input {
    flex: 1;
    min-width: 0;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
  }

  .add {
    align-self: flex-start;
  }

  .error {
    margin: 6px 0 0;
    font-size: 13px;
  }
</style>

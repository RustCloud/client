<script lang="ts">
  import { open as pickFolder } from '@tauri-apps/plugin-dialog';
  import { homeDir, join } from '@tauri-apps/api/path';
  import * as api from './api';

  let {
    synced,
    ondone,
    oncancel,
    onexpired,
  }: { synced: string[]; ondone: () => void; oncancel: () => void; onexpired: () => void } = $props();

  // 1. The folder on this computer.
  let local = $state('');
  let localInfo = $state<api.LocalFolder | null>(null);

  // 2. The folder in the cloud.
  let listing = $state<api.Listing | null>(null);
  let newName = $state('');
  let creating = $state(false);

  let error = $state('');
  let busy = $state(false);

  let folders = $derived(listing ? listing.children.filter((n) => n.is_dir && !n.name.startsWith('.')) : []);
  let atRoot = $derived(!listing || listing.folder.parent_id === null);
  let remoteTaken = $derived(
    !!listing && [listing.folder, ...listing.path].some((n) => synced.includes(n.id)),
  );
  let localOk = $derived(!!local.trim() && !!localInfo && (!localInfo.exists || localInfo.is_dir));
  let ready = $derived(localOk && !atRoot && !remoteTaken && !busy);

  function fail(e: unknown) {
    const err = api.asError(e);
    if (err.needs_login) onexpired();
    else error = err.message;
  }

  async function openRemote(folder: string | null) {
    error = '';
    try {
      listing = await api.list(folder);
    } catch (e) {
      fail(e);
    }
  }

  $effect(() => {
    openRemote(null);
  });

  // What is at the typed path, a moment after typing stops.
  $effect(() => {
    const path = local.trim();
    localInfo = null;
    if (!path) return;
    const timer = setTimeout(async () => {
      try {
        localInfo = await api.localFolder(path);
      } catch (e) {
        fail(e);
      }
    }, 250);
    return () => clearTimeout(timer);
  });

  function baseName(path: string): string {
    return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? '';
  }

  async function chooseLocal() {
    let defaultPath: string | undefined;
    try {
      defaultPath = local.trim() || (await homeDir());
    } catch {
      /* no home: the dialog picks */
    }
    const picked = await pickFolder({ directory: true, title: 'Папка на этом компьютере', defaultPath });
    if (typeof picked !== 'string') return;
    local = picked;
    if (!newName) newName = baseName(picked);
  }

  async function suggestLocal() {
    try {
      local = await join(await homeDir(), 'RustCloud');
    } catch (e) {
      fail(e);
    }
  }

  async function makeRemote() {
    if (!listing || !newName.trim()) return;
    creating = true;
    error = '';
    try {
      const made = await api.createFolder(listing.folder.id, newName.trim());
      newName = '';
      await openRemote(made.id);
    } catch (e) {
      if (api.asError(e).code === 'NAME_TAKEN') error = `«${newName.trim()}» здесь уже есть — откройте её или выберите другое имя`;
      else fail(e);
    } finally {
      creating = false;
    }
  }

  async function start() {
    if (!listing || !ready) return;
    busy = true;
    error = '';
    try {
      await api.addSyncPair(local.trim(), listing.folder.id);
      ondone();
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }

  function localHint(info: api.LocalFolder | null): string {
    if (!local.trim()) return 'Выберите папку или впишите путь к новой — она будет создана.';
    if (!info) return '';
    if (!info.exists) return 'Такой папки нет — она будет создана.';
    if (!info.is_dir) return 'Это файл, а не папка.';
    if (info.entries === 0) return 'Пустая папка — в неё скачается папка из облака.';
    return `В папке ${info.entries} элементов — они объединятся с папкой в облаке (одинаковые имена с разным содержимым сохранятся обе версии).`;
  }
</script>

<section class="add">
  <h2>Новая синхронизируемая папка</h2>

  <div class="step">
    <h3>1. Папка на этом компьютере</h3>
    <div class="line">
      <input bind:value={local} placeholder="/home/…/RustCloud" spellcheck="false" />
      <button onclick={chooseLocal}>Выбрать…</button>
    </div>
    <p class="hint" class:error={!!localInfo && localInfo.exists && !localInfo.is_dir}>
      {localHint(localInfo)}
      {#if !local.trim()}<button class="link" onclick={suggestLocal}>Предложить ~/RustCloud</button>{/if}
    </p>
  </div>

  <div class="step">
    <h3>2. Папка в облаке</h3>
    <nav aria-label="Путь в облаке">
      <button class="crumb" onclick={() => openRemote(null)}>Облако</button>
      {#if listing}
        {#each listing.path.slice(1) as dir (dir.id)}
          <span class="muted">/</span>
          <button class="crumb" onclick={() => openRemote(dir.id)}>{dir.name}</button>
        {/each}
        {#if !atRoot}
          <span class="muted">/</span>
          <span class="current">{listing.folder.name}</span>
        {/if}
      {/if}
    </nav>
    <ul class="folders">
      {#each folders as dir (dir.id)}
        <li>
          <button class="row" onclick={() => openRemote(dir.id)}>
            <span aria-hidden="true">📁</span>
            <span class="name">{dir.name}</span>
            {#if synced.includes(dir.id)}<span class="muted tag">уже синхронизируется</span>{/if}
          </button>
        </li>
      {:else}
        {#if listing}<li class="muted none">Вложенных папок нет</li>{/if}
      {/each}
    </ul>
    <div class="line">
      <input
        bind:value={newName}
        placeholder="Имя новой папки"
        onkeydown={(e) => e.key === 'Enter' && makeRemote()}
      />
      <button onclick={makeRemote} disabled={!newName.trim() || creating}>Создать здесь</button>
    </div>
    <p class="hint" class:error={remoteTaken}>
      {#if atRoot}
        Откройте папку или создайте новую — всё облако целиком не синхронизируется.
      {:else if remoteTaken}
        Эта папка (или папка выше) уже синхронизируется.
      {:else if listing}
        Синхронизироваться будет <b>/{[...listing.path.slice(1), listing.folder].map((n) => n.name).join('/')}</b> со
        всем содержимым.
      {/if}
    </p>
  </div>

  {#if error}<p class="error">{error}</p>{/if}

  <div class="actions">
    <button class="primary" onclick={start} disabled={!ready}>{busy ? 'Добавление…' : 'Синхронизировать'}</button>
    <button onclick={oncancel}>Отмена</button>
  </div>
</section>

<style>
  .add {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    padding: 12px 14px;
    margin-bottom: 14px;
    display: grid;
    gap: 12px;
  }

  h2 {
    font-size: 16px;
    margin: 0;
  }

  h3 {
    font-size: 14px;
    margin: 0 0 6px;
  }

  .line {
    display: flex;
    gap: 8px;
  }

  .line input {
    flex: 1;
    min-width: 0;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
  }

  .hint {
    margin: 6px 0 0;
    font-size: 13px;
    color: var(--muted);
  }

  .hint.error {
    color: var(--danger);
  }

  button.link {
    border: none;
    background: none;
    padding: 0 4px;
    color: var(--accent);
    text-decoration: underline;
  }

  nav {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px;
    margin-bottom: 4px;
  }

  .crumb {
    border: none;
    background: none;
    padding: 2px 6px;
  }

  .current {
    padding: 2px 6px;
    font-weight: 600;
  }

  .folders {
    list-style: none;
    margin: 0 0 8px;
    padding: 0;
    max-height: 220px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    border: none;
    border-radius: 0;
    background: none;
    text-align: left;
    padding: 6px 8px;
  }

  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tag {
    font-size: 12px;
  }

  .none {
    padding: 6px 8px;
  }

  .actions {
    display: flex;
    gap: 8px;
  }
</style>

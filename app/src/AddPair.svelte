<script lang="ts">
  import * as api from './api';
  import Pane, { type Crumb, type Item } from './Pane.svelte';

  let {
    pairs,
    ondone,
    oncancel,
    onexpired,
  }: { pairs: api.Pair[]; ondone: () => void; oncancel: (() => void) | null; onexpired: () => void } = $props();

  function message(e: unknown): string {
    const err = api.asError(e);
    if (err.needs_login) onexpired();
    return err.message;
  }

  // ---- This computer: `null` is the list of places (home, drives).
  let places = $state<api.LocalDir[]>([]);
  let localAt = $state<string | null>(null);
  let localDirs = $state<api.LocalDir[]>([]);
  let local = $state<Item | null>(null);
  let localError = $state('');

  async function openLocal(path: string | null) {
    localError = '';
    try {
      localDirs = path === null ? places : await api.localDirs(path);
      localAt = path;
    } catch (e) {
      localError = message(e);
    }
  }

  const syncedLocal = (path: string) => pairs.some((p) => p.local === path);
  const localItem = (d: api.LocalDir): Item => ({
    key: d.path,
    name: d.name,
    note: syncedLocal(d.path) ? 'синхронизируется' : undefined,
  });

  /** `/a/b` → `/`, `/a`, `/a/b`; `C:\a` → `C:\`, `C:\a`. */
  function localCrumbs(path: string | null): Crumb[] {
    const crumbs: Crumb[] = [{ label: 'Места', go: () => openLocal(null) }];
    if (path === null) return crumbs;
    const windows = /^[A-Za-z]:\\/.test(path);
    const sep = windows ? '\\' : '/';
    const parts = path.split(sep).filter(Boolean);
    let at = windows ? '' : '/';
    if (!windows) crumbs.push({ label: '/', go: () => openLocal('/') });
    parts.forEach((part, i) => {
      at = windows ? (i === 0 ? part + '\\' : at + (at.endsWith('\\') ? '' : '\\') + part) : at + (i ? '/' : '') + part;
      const target = at;
      crumbs.push({ label: part, go: () => openLocal(target) });
    });
    return crumbs;
  }

  function localName(path: string): string {
    return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
  }

  async function createLocal(name: string) {
    if (localAt === null) return;
    localError = '';
    try {
      const made = await api.createLocalDir(localAt, name);
      await openLocal(localAt);
      local = localItem(made);
    } catch (e) {
      localError = message(e);
      throw e;
    }
  }

  // ---- The cloud.
  let listing = $state<api.Listing | null>(null);
  let remote = $state<Item | null>(null);
  let remoteError = $state('');

  async function openRemote(folder: string | null) {
    remoteError = '';
    try {
      listing = await api.list(folder);
    } catch (e) {
      remoteError = message(e);
    }
  }

  /** The path of a folder in the cloud, for showing. */
  function cloudPath(l: api.Listing, node: api.Node): string {
    const names = [...l.path.slice(1), l.folder].filter((n) => n.parent_id !== null).map((n) => n.name);
    if (node.id !== l.folder.id) names.push(node.name);
    return '/' + names.join('/');
  }

  const syncedRemote = (id: string) => pairs.some((p) => p.remote === id);
  const remoteItem = (n: api.Node): Item => ({
    key: n.id,
    name: n.parent_id === null ? 'Всё облако' : n.name,
    note: syncedRemote(n.id) ? 'синхронизируется' : undefined,
  });
  let remotePaths = $state<Record<string, string>>({});

  function selectRemote(item: Item) {
    if (!listing) return;
    const node = item.key === listing.folder.id ? listing.folder : listing.children.find((n) => n.id === item.key);
    if (node) remotePaths[item.key] = cloudPath(listing, node);
    remote = item;
  }

  async function createRemote(name: string) {
    if (!listing) return;
    remoteError = '';
    try {
      const made = await api.createFolder(listing.folder.id, name);
      await openRemote(listing.folder.id);
      if (listing) selectRemote(remoteItem(made));
    } catch (e) {
      remoteError = api.asError(e).code === 'NAME_TAKEN' ? `«${name}» здесь уже есть` : message(e);
      throw e;
    }
  }

  let remoteCrumbs = $derived<Crumb[]>(
    listing
      ? [
          { label: 'Облако', go: () => openRemote(null) },
          ...[...listing.path.slice(1), ...(listing.folder.parent_id ? [listing.folder] : [])].map((n) => ({
            label: n.name,
            go: () => openRemote(n.id),
          })),
        ]
      : [{ label: 'Облако', go: () => openRemote(null) }],
  );

  // ---- Both.
  let busy = $state(false);
  let error = $state('');
  let ready = $derived(!!local && !!remote && !busy);

  async function start() {
    if (!local || !remote) return;
    busy = true;
    error = '';
    try {
      await api.addSyncPair(local.key, remote.key);
      local = null;
      remote = null;
      ondone();
    } catch (e) {
      error = message(e);
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    (async () => {
      try {
        places = await api.localPlaces();
      } catch (e) {
        localError = message(e);
      }
      await openLocal(places[0]?.path ?? null);
    })();
    openRemote(null);
  });
</script>

<section class="add">
  <div class="panes">
    <Pane
      title="На этом компьютере"
      icon="💻"
      crumbs={localCrumbs(localAt)}
      here={localAt === null ? null : localItem({ name: localName(localAt), path: localAt })}
      items={localDirs.map(localItem)}
      selected={local?.key ?? null}
      error={localError}
      onselect={(item) => (local = item)}
      onopen={(item) => openLocal(item.key)}
      oncreate={localAt === null ? null : createLocal}
    />
    <div class="arrow" aria-hidden="true">⇄</div>
    <Pane
      title="В облаке"
      icon="☁"
      crumbs={remoteCrumbs}
      here={listing ? remoteItem(listing.folder) : null}
      items={listing ? listing.children.filter((n) => n.is_dir && !n.name.startsWith('.')).map(remoteItem) : []}
      selected={remote?.key ?? null}
      error={remoteError}
      onselect={selectRemote}
      onopen={(item) => openRemote(item.key)}
      oncreate={listing ? createRemote : null}
    />
  </div>

  <div class="choice">
    <span class:muted={!local}>{local ? local.key : 'Выберите папку на компьютере'}</span>
    <span aria-hidden="true">⇄</span>
    <span class:muted={!remote}>{remote ? (remotePaths[remote.key] ?? remote.name) : 'Выберите папку в облаке'}</span>
  </div>
  {#if error}<p class="error">{error}</p>{/if}
  <div class="actions">
    <button class="primary" onclick={start} disabled={!ready}>{busy ? 'Добавление…' : 'Синхронизировать'}</button>
    {#if oncancel}<button onclick={oncancel}>Отмена</button>{/if}
  </div>
</section>

<style>
  .add {
    display: grid;
    gap: 10px;
    margin-bottom: 18px;
  }

  .panes {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    gap: 8px;
    align-items: center;
  }

  .arrow {
    font-size: 20px;
    color: var(--muted);
  }

  @media (max-width: 640px) {
    .panes {
      grid-template-columns: 1fr;
    }
    .arrow {
      text-align: center;
    }
  }

  .choice {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    font-weight: 600;
    word-break: break-all;
  }

  .actions {
    display: flex;
    gap: 8px;
  }
</style>

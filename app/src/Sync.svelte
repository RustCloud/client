<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { ask } from '@tauri-apps/plugin-dialog';
  import * as api from './api';
  import * as format from './format';
  import AddPair from './AddPair.svelte';

  let { onexpired }: { onexpired: () => void } = $props();

  let pairs = $state<api.Pair[]>([]);
  let error = $state('');
  let adding = $state(false);
  let loaded = $state(false);
  let paused = $state(false);
  let unlisten: UnlistenFn[] = [];

  async function load() {
    try {
      [pairs, paused] = await Promise.all([api.syncPairs(), api.syncPaused()]);
      loaded = true;
    } catch (e) {
      const err = api.asError(e);
      if (err.needs_login) onexpired();
      else error = err.message;
    }
  }

  onMount(async () => {
    await load();
    unlisten.push(
      await listen<{ id: string; status: api.PairStatus }>('sync-status', (event) => {
        const pair = pairs.find((p) => p.id === event.payload.id);
        if (pair) pair.status = event.payload.status;
        if (event.payload.status.needs_login) onexpired();
      }),
      // Paused or resumed here or from the tray.
      await listen<boolean>('sync-paused', load),
    );
  });

  onDestroy(() => unlisten.forEach((u) => u()));

  async function remove(pair: api.Pair) {
    const sure = await ask(`Перестать синхронизировать ${pair.local}? Файлы останутся и здесь, и на сервере.`, {
      title: 'RustCloud',
      kind: 'warning',
    });
    if (!sure) return;
    await api.removeSyncPair(pair.id);
    pairs = pairs.filter((p) => p.id !== pair.id);
  }

  function summary(s: api.PairStatus): string {
    if (s.state === 'paused') return 'Приостановлено';
    if (s.state === 'syncing') return 'Синхронизация…';
    if (s.state === 'error') return s.error ?? 'Ошибка';
    if (!s.last_sync) return 'Ожидает первой синхронизации';
    const moved = [
      s.uploaded && `↑ ${s.uploaded}`,
      s.downloaded && `↓ ${s.downloaded}`,
      s.deleted && `удалено ${s.deleted}`,
    ].filter(Boolean);
    return `Синхронизировано ${format.date(s.last_sync)}${moved.length ? ' · ' + moved.join(' · ') : ''}`;
  }
</script>

<main>
  {#if error}
    <p class="error">{error}</p>
  {/if}
  {#if !loaded}
    <p class="muted">Загрузка…</p>
  {:else if adding || pairs.length === 0}
    <AddPair
      {pairs}
      ondone={async () => ((adding = false), await load())}
      oncancel={pairs.length ? () => (adding = false) : null}
      {onexpired}
    />
  {:else}
    <div class="top">
      <button class="primary" onclick={() => (adding = true)}>Добавить пару папок…</button>
    </div>
  {/if}
  {#if pairs.length}<h2>Синхронизируемые папки</h2>{/if}
  {#if paused}
    <div class="paused">
      Синхронизация приостановлена.
      <button onclick={() => api.setSyncPaused(false)}>Возобновить</button>
    </div>
  {:else if pairs.length}
    <div class="top">
      <button onclick={() => api.setSyncPaused(true)}>Приостановить всё</button>
    </div>
  {/if}
  <ul>
    {#each pairs as pair (pair.id)}
      <li class="pair" class:failed={pair.status.state === 'error'}>
        <div class="paths">
          <div class="local" title={pair.local}>{pair.local}</div>
          <div class="muted">⇅ облако: {pair.remote_path === '/' ? 'всё облако' : pair.remote_path}</div>
        </div>
        <div class="status" class:error={pair.status.state === 'error'}>{summary(pair.status)}</div>
        {#if pair.status.confirm_deletions}
          <div class="warn">
            Если продолжить, будет удалено {pair.status.confirm_deletions[0]} из {pair.status.confirm_deletions[1]} файлов
            и папок (на сервере — в корзину). Проверьте, что так и задумано.
            <button class="danger" onclick={() => api.confirmDeletions(pair.id)}>Удалить</button>
          </div>
        {/if}
        {#if pair.status.conflicts.length}
          <details>
            <summary>Конфликты: {pair.status.conflicts.length}</summary>
            <p class="muted">Ваша версия сохранена рядом как «… (conflict дата)».</p>
            <ul class="small">
              {#each pair.status.conflicts as path (path)}<li>{path}</li>{/each}
            </ul>
          </details>
        {/if}
        {#if pair.status.failed.length}
          <details>
            <summary class="error">Не получилось: {pair.status.failed.length}</summary>
            <ul class="small">
              {#each pair.status.failed as [path, why] (path)}<li>{path}: {why}</li>{/each}
            </ul>
          </details>
        {/if}
        <div class="actions">
          <button onclick={() => api.syncNow(pair.id)} disabled={pair.status.state === 'syncing' || paused}>Синхронизировать сейчас</button>
          <button onclick={() => remove(pair)}>Убрать</button>
        </div>
      </li>
    {/each}
  </ul>
</main>

<style>
  main {
    padding: 12px 16px 48px;
  }

  .top {
    margin-bottom: 12px;
  }

  h2 {
    font-size: 15px;
    margin: 0 0 8px;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .pair {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    padding: 12px;
    margin-bottom: 10px;
    display: grid;
    gap: 8px;
  }

  .pair.failed {
    border-color: var(--danger);
  }

  .local {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .paused {
    border-left: 3px solid var(--muted);
    padding: 6px 10px;
    margin-bottom: 12px;
    display: flex;
    gap: 12px;
    align-items: center;
    flex-wrap: wrap;
  }

  .warn {
    border-left: 3px solid var(--danger);
    padding: 6px 10px;
  }

  .warn button {
    margin-top: 6px;
    display: block;
  }

  button.danger {
    border-color: var(--danger);
    color: var(--danger);
  }

  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .small {
    font-size: 13px;
    padding-left: 12px;
  }

  summary {
    cursor: pointer;
  }
</style>

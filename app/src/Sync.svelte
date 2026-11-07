<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { ask } from '@tauri-apps/plugin-dialog';
  import * as api from './api';
  import * as format from './format';

  let { onexpired }: { onexpired: () => void } = $props();

  let pairs = $state<api.Pair[]>([]);
  let error = $state('');
  let unlisten: UnlistenFn | null = null;

  async function load() {
    try {
      pairs = await api.syncPairs();
    } catch (e) {
      const err = api.asError(e);
      if (err.needs_login) onexpired();
      else error = err.message;
    }
  }

  onMount(async () => {
    await load();
    unlisten = await listen<{ id: string; status: api.PairStatus }>('sync-status', (event) => {
      const pair = pairs.find((p) => p.id === event.payload.id);
      if (pair) pair.status = event.payload.status;
      if (event.payload.status.needs_login) onexpired();
    });
  });

  onDestroy(() => unlisten?.());

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
  {#if pairs.length === 0}
    <p class="muted empty">
      Синхронизируемых папок нет. Откройте папку во вкладке «Файлы» и нажмите «Синхронизировать с папкой…».
    </p>
  {/if}
  <ul>
    {#each pairs as pair (pair.id)}
      <li class="pair" class:failed={pair.status.state === 'error'}>
        <div class="paths">
          <div class="local" title={pair.local}>{pair.local}</div>
          <div class="muted">⇅ {pair.remote_path}</div>
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
          <button onclick={() => api.syncNow(pair.id)} disabled={pair.status.state === 'syncing'}>Синхронизировать сейчас</button>
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

  .empty {
    max-width: 480px;
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

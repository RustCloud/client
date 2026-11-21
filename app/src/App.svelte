<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import * as api from './api';
  import Access from './Access.svelte';
  import Login from './Login.svelte';
  import Files from './Files.svelte';
  import Sync from './Sync.svelte';

  let account = $state<api.Account | null>(null);
  let loading = $state(true);
  let error = $state('');
  let tab = $state<'files' | 'sync' | 'access'>('files');
  // The "Доступ" tab: only where there is a phone account (Android).
  let phoneAccount = $state(false);
  let unlisten: UnlistenFn | undefined;

  onMount(async () => {
    // The account was removed in Android's settings: the app logged out.
    unlisten = await listen('logged-out', () => {
      account = null;
      tab = 'files';
    });
    api.accountApps().then((apps) => (phoneAccount = apps !== null)).catch(() => {});
    try {
      account = await api.session();
    } catch (e) {
      // Offline or the server is down: the login screen says why.
      error = api.asError(e).message;
    } finally {
      loading = false;
    }
  });

  onDestroy(() => unlisten?.());

  async function logout() {
    await api.logout();
    account = null;
  }

  function expired() {
    account = null;
    error = 'Вход истёк — войдите снова';
  }
</script>

{#if loading}
  <p class="center muted">Загрузка…</p>
{:else if account}
  <div class="bar">
    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={tab === 'files'} class:active={tab === 'files'} onclick={() => (tab = 'files')}>
        Файлы
      </button>
      <button role="tab" aria-selected={tab === 'sync'} class:active={tab === 'sync'} onclick={() => (tab = 'sync')}>
        Синхронизация
      </button>
      {#if phoneAccount}
        <button
          role="tab"
          aria-selected={tab === 'access'}
          class:active={tab === 'access'}
          onclick={() => (tab = 'access')}
        >
          Доступ
        </button>
      {/if}
    </div>
    <div class="account">
      <span class="muted" title={account.server}>{account.display_name || account.username}</span>
      <button onclick={logout}>Выйти</button>
    </div>
  </div>
  {#if tab === 'files'}
    <Files onexpired={expired} />
  {:else if tab === 'sync'}
    <Sync onexpired={expired} />
  {:else}
    <Access {account} />
  {/if}
{:else}
  <Login {error} onlogin={(a) => ((account = a), (error = ''))} />
{/if}

<style>
  .center {
    text-align: center;
    margin-top: 30vh;
  }

  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 16px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  .tabs {
    display: flex;
    gap: 4px;
  }

  .tabs button {
    border: none;
    background: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
  }

  .tabs button.active {
    border-bottom-color: var(--accent);
    font-weight: 600;
  }

  .account {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }

  .account span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>

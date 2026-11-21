<script lang="ts">
  // Android: which apps use the phone's RustCloud account (app/src-tauri/src/account.rs).
  import { onMount } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import * as api from './api';

  let { account }: { account: api.Account } = $props();

  let apps = $state<api.AllowedApp[]>([]);
  let error = $state('');

  async function load() {
    try {
      apps = (await api.accountApps()) ?? [];
    } catch (e) {
      error = api.asError(e).message;
    }
  }

  onMount(load);

  async function revoke(app: api.AllowedApp) {
    const sure = await ask(`Запретить «${app.label}» доступ к аккаунту RustCloud?`, {
      title: 'RustCloud',
      kind: 'warning',
    });
    if (!sure) return;
    try {
      await api.accountRevoke(app.package);
      await load();
    } catch (e) {
      error = api.asError(e).message;
    }
  }
</script>

<main>
  <h2>Аккаунт телефона</h2>
  <p>
    Вход <b>{account.username}</b> на {account.server} есть в настройках Android: «Аккаунты → RustCloud». Другие
    приложения RustCloud входят через него сами, без пароля. Удалить аккаунт там — то же, что «Выйти» здесь.
  </p>

  <h2>Приложения других разработчиков</h2>
  {#if error}
    <p class="error">{error}</p>
  {/if}
  {#if apps.length === 0}
    <p class="muted">
      Нет. Если стороннее приложение попросит доступ к аккаунту, телефон покажет предупреждение, и решать будете вы.
    </p>
  {:else}
    <p class="muted">
      Им разрешён доступ к вашим файлам. После запрета приложение теряет доступ самое большее через 10 минут.
    </p>
    <ul>
      {#each apps as app (app.package)}
        <li>
          <div class="name">
            <div>{app.label}</div>
            <div class="muted small">{app.package}</div>
          </div>
          <button class="danger" onclick={() => revoke(app)}>Запретить</button>
        </li>
      {/each}
    </ul>
  {/if}
</main>

<style>
  main {
    padding: 12px 16px 48px;
  }

  h2 {
    font-size: 15px;
    margin: 16px 0 8px;
  }

  h2:first-child {
    margin-top: 0;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    padding: 10px 12px;
    margin-bottom: 8px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .name {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .small {
    font-size: 13px;
  }

  button.danger {
    border-color: var(--danger);
    color: var(--danger);
  }
</style>

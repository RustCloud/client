<script lang="ts">
  import * as api from './api';

  let { error: startError = '', onlogin }: { error?: string; onlogin: (account: api.Account) => void } = $props();

  let server = $state('');
  let waiting = $state(false);
  let error = $state('');
  let shown = $derived(error || startError);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    waiting = true;
    error = '';
    try {
      onlogin(await api.login(server));
    } catch (e) {
      error = api.asError(e).message;
    } finally {
      waiting = false;
    }
  }
</script>

<main>
  <h1>RustCloud</h1>
  <form onsubmit={submit}>
    <label for="server">Адрес сервера</label>
    <input id="server" bind:value={server} placeholder="cloud.example.com" autocomplete="url" disabled={waiting} />
    <button class="primary" type="submit" disabled={waiting || !server.trim()}>Войти через браузер</button>
  </form>
  {#if waiting}
    <p class="muted">Войдите в открывшемся браузере и разрешите доступ этому устройству.</p>
  {/if}
  {#if shown}
    <p class="error">{shown}</p>
  {/if}
</main>

<style>
  main {
    max-width: 380px;
    margin: 18vh auto 0;
    padding: 0 16px;
  }

  h1 {
    font-size: 24px;
    margin: 0 0 20px;
  }

  form {
    display: grid;
    gap: 10px;
  }

  label {
    font-weight: 600;
  }
</style>

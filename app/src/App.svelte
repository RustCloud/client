<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from './api';
  import Login from './Login.svelte';
  import Files from './Files.svelte';

  let account = $state<api.Account | null>(null);
  let loading = $state(true);
  let error = $state('');

  onMount(async () => {
    try {
      account = await api.session();
    } catch (e) {
      // Offline or the server is down: the login screen says why.
      error = api.asError(e).message;
    } finally {
      loading = false;
    }
  });

  async function logout() {
    await api.logout();
    account = null;
  }
</script>

{#if loading}
  <p class="center muted">Загрузка…</p>
{:else if account}
  <Files {account} onlogout={logout} onexpired={() => (account = null)} />
{:else}
  <Login {error} onlogin={(a) => (account = a)} />
{/if}

<style>
  .center {
    text-align: center;
    margin-top: 30vh;
  }
</style>

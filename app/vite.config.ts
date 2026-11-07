import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

// `npm run tauri dev` starts this on :1420 and points the window at it;
// `npm run build` writes app/dist, which the app embeds.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: 'es2022' },
});

// The commands of the Rust side (app/src-tauri/src/lib.rs).
import { invoke } from '@tauri-apps/api/core';

export interface Node {
  id: string;
  parent_id: string | null;
  name: string;
  is_dir: boolean;
  size: number;
  mime_type: string | null;
  etag: string | null;
  updated_at: string;
  modified_at: string | null;
}

export interface Account {
  server: string;
  username: string;
  display_name: string;
}

export interface Listing {
  folder: Node;
  path: Node[];
  children: Node[];
}

export interface CmdError {
  code: string | null;
  message: string;
  needs_login: boolean;
}

export function asError(e: unknown): CmdError {
  if (e && typeof e === 'object' && 'message' in e) return e as CmdError;
  return { code: null, message: String(e), needs_login: false };
}

export const session = () => invoke<Account | null>('session');
export const login = (server: string) => invoke<Account>('login', { server });
export const logout = () => invoke<void>('logout');
export const list = (folder: string | null) => invoke<Listing>('list', { folder });

// Synced folders (app/src-tauri/src/sync.rs).

export interface PairStatus {
  state: 'idle' | 'syncing' | 'error' | 'paused';
  last_sync: string | null;
  uploaded: number;
  downloaded: number;
  deleted: number;
  conflicts: string[];
  failed: [string, string][];
  error: string | null;
  confirm_deletions: [number, number] | null;
  needs_login: boolean;
}

export interface Pair {
  id: string;
  server: string;
  local: string;
  remote: string;
  remote_path: string;
  status: PairStatus;
}

export const syncPairs = () => invoke<Pair[]>('sync_pairs');
export const syncPaused = () => invoke<boolean>('sync_paused');
export const setSyncPaused = (paused: boolean) => invoke<void>('set_sync_paused', { paused });
export const addSyncPair = (local: string, remote: string) => invoke<Pair>('add_sync_pair', { local, remote });
export const removeSyncPair = (id: string) => invoke<void>('remove_sync_pair', { id });
export const syncNow = (id: string) => invoke<void>('sync_now', { id });
export const confirmDeletions = (id: string) => invoke<void>('confirm_deletions', { id });

export interface LocalDir {
  name: string;
  path: string;
}

export const createFolder = (parent: string, name: string) => invoke<Node>('create_folder', { parent, name });
export const localDirs = (path: string) => invoke<LocalDir[]>('local_dirs', { path });
export const localPlaces = () => invoke<LocalDir[]>('local_places');
export const createLocalDir = (parent: string, name: string) => invoke<LocalDir>('create_local_dir', { parent, name });

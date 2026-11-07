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

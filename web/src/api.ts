// Le client de l'API du Core : JSON, cookie de session du même site, erreurs avec leur statut HTTP.
import { create } from 'zustand';

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

export interface PublicUser {
  id: string;
  username: string;
  displayName: string;
  admin: boolean;
  groups: string[];
  disabled: boolean;
  lang: string;
  theme: string;
  sso: boolean;
}

export interface AuthState {
  needsSetup: boolean;
  user: PublicUser | null;
  serverName: string;
  timezone: string;
  version: string;
  sso: string | null;
}

/** L'état de l'interface : la connexion, et la liaison avec le serveur. */
export const useApp = create<{ auth: AuthState | null; connected: boolean }>(() => ({ auth: null, connected: true }));

export async function api<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, { method, credentials: 'same-origin', headers: body === undefined ? {} : { 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
  } catch {
    throw new ApiError(0, 'the server does not answer');
  }
  if (res.status === 401 && !path.startsWith('/api/auth/')) {
    // Session expirée : retour à la page de connexion.
    const a = useApp.getState().auth;
    if (a?.user) useApp.setState({ auth: { ...a, user: null } });
  }
  if (!res.ok) throw new ApiError(res.status, (await res.text()) || res.statusText);
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

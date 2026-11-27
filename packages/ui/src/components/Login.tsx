/*
 * La page de connexion ; au premier démarrage (aucun compte), elle crée le compte administrateur.
 * Repris de LiveCut. La connexion elle-même est faite par l'appelant (onLogin / onSetup), qui rejette
 * avec une erreur portant le statut HTTP (`status`) : la page en tire le message.
 */
import { useEffect, useState } from 'react';
import { KeyRound, LogIn, ShieldCheck, UserPlus } from 'lucide-react';
import { LANGS, setLang, t, useLang } from '../i18n';
import { Brand } from './Brand';

/** L'erreur SSO renvoyée par le serveur dans l'adresse (?sso_error=…&sso_detail=…), puis l'adresse nettoyée. */
function ssoError(): string | null {
  const q = new URLSearchParams(location.search);
  const code = q.get('sso_error');
  if (!code) return null;
  const detail = q.get('sso_detail') ?? '';
  q.delete('sso_error');
  q.delete('sso_detail');
  const rest = q.toString();
  history.replaceState(null, '', location.pathname + (rest ? `?${rest}` : ''));
  switch (code) {
    case 'noGroup':
      return t('None of your directory groups gives access to Strata.');
    case 'disabled':
      return t('This account is disabled.');
    case 'domain':
      return t('This account is not in a domain allowed to sign in.');
    case 'expired':
      return t('Signing in took too long: try again.');
    default:
      return t('The directory refused the sign-in: {detail}', { detail });
  }
}

export function Login({ module, serverName, setup = false, sso, ssoHref, notice, cliHint, onLogin, onSetup }: {
  /** Le module affiché sous « Strata » (« Core »). */
  module: string;
  /** Le nom d'usage du serveur, sous la marque. */
  serverName?: string;
  /** Premier démarrage : crée le compte administrateur. */
  setup?: boolean;
  /** Le nom du fournisseur d'annuaire (« Okta ») quand le SSO est actif. */
  sso?: string | null;
  /** L'adresse qui lance la connexion par l'annuaire. */
  ssoHref?: string;
  /** Un message à afficher d'emblée (« Connecté sur un autre ordinateur. »). */
  notice?: string | null;
  /** La commande qui crée le compte depuis le serveur (premier démarrage, à distance). */
  cliHint?: string;
  onLogin: (username: string, password: string) => Promise<unknown>;
  onSetup?: (username: string, password: string, displayName: string) => Promise<unknown>;
}) {
  const lang = useLang((s) => s.lang);
  const [username, setUsername] = useState(setup ? 'admin' : '');
  const [displayName, setDisplayName] = useState('');
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const e = ssoError();
    if (e) setError(e);
    else if (notice) setError(notice);
  }, [notice]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (busy) return;
    if (setup && password !== repeat) {
      setError(t('The passwords do not match.'));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (setup && onSetup) await onSetup(username.trim(), password, displayName.trim());
      else await onLogin(username.trim(), password);
    } catch (err) {
      const status = (err as { status?: number }).status ?? 0;
      if (!setup && status === 401) setError(t('Wrong user name or password.'));
      else if (status === 429) setError(t('Too many tries: wait a moment, then try again.'));
      else if (!setup && status === 403 && sso) setError(t('Sign in with {name}', { name: sso }));
      else setError((err as Error).message ? t((err as Error).message) : t('The server does not answer.'));
    }
    setBusy(false);
  };

  const empty = !username.trim() || !password || (setup && !repeat);

  return (
    <div className="st-login">
      <div className="st-choice st-login-lang" role="group" aria-label={t('Language')}>
        {LANGS.map((l) => (
          <button key={l.id} type="button" className={lang === l.id ? 'on' : ''} aria-pressed={lang === l.id} title={l.name} onClick={() => setLang(l.id)}>
            {l.label}
          </button>
        ))}
      </div>
      <form className="st-login-card" onSubmit={submit}>
        <div className="st-login-head">
          <Brand module={module} stacked />
          {serverName && <p className="st-muted">{serverName}</p>}
        </div>
        {setup ? (
          <div className="st-login-intro">
            <ShieldCheck size={16} />
            <span>{t('Welcome! Create the administrator account.')}</span>
          </div>
        ) : (
          sso &&
          ssoHref && (
            <>
              <a className="st-btn primary block st-sso-btn" href={ssoHref}>
                <KeyRound size={15} /> {t('Sign in with {name}', { name: sso })}
              </a>
              <div className="st-login-sep">{t('local account')}</div>
            </>
          )
        )}
        <label className="st-field">
          <span className="st-field-label">{t('User name')}</span>
          <input value={username} autoFocus={!setup} autoComplete="username" autoCapitalize="none" spellCheck={false} onChange={(e) => setUsername(e.target.value)} />
        </label>
        {setup && (
          <label className="st-field">
            <span className="st-field-label">{t('Display name')}</span>
            <input value={displayName} placeholder={t('Optional')} autoComplete="name" onChange={(e) => setDisplayName(e.target.value)} />
          </label>
        )}
        <label className="st-field">
          <span className="st-field-label">{t('Password')}</span>
          <input type="password" value={password} autoFocus={setup} autoComplete={setup ? 'new-password' : 'current-password'} onChange={(e) => setPassword(e.target.value)} />
        </label>
        {setup && (
          <label className="st-field">
            <span className="st-field-label">{t('Repeat the password')}</span>
            <input type="password" value={repeat} autoComplete="new-password" onChange={(e) => setRepeat(e.target.value)} />
            <span className="st-field-hint">{t('At least 8 characters.')}</span>
          </label>
        )}
        <button type="submit" className={`st-btn block ${setup || !sso ? 'primary' : ''}`} disabled={empty || busy}>
          {setup ? <UserPlus size={15} /> : <LogIn size={15} />} {setup ? t('Create the administrator account') : t('Sign in')}
        </button>
        {error && (
          <div className="st-login-error" role="alert">
            {error}
          </div>
        )}
        {setup && cliHint && (
          <p className="st-field-hint">
            {t('For safety, this page only works on the server itself (http://localhost). Remotely, run this on the server:')} <code>{cliHint}</code>
          </p>
        )}
      </form>
    </div>
  );
}

// Réglages › Personnes › Annuaire : la connexion par l'annuaire de l'entreprise (OpenID Connect).
import { useEffect, useState } from 'react';
import { Copy, KeyRound, Send, ShieldCheck } from 'lucide-react';
import { Card, Dropdown, Field, FieldGrid, PageHead, SaveBar, Segmented, Switch, t, tn, toast } from '@strata/ui';
import { api } from '../api';
import { friendly } from '../errors';
import { TagInput } from './TagInput';

type LocalAccounts = 'none' | 'adminOnly' | 'all';

interface Sso {
  enabled: boolean;
  provider: string;
  issuer: string;
  clientId: string;
  clientSecret: string;
  groupsClaim: string;
  adminGroups: string[];
  allowAny: boolean;
  localAccounts: LocalAccounts;
  domains: string[];
}

interface SsoAnswer {
  sso: Sso;
  redirectUri: string;
  buttonName: string;
}

const PROVIDERS = [
  { value: 'entra', label: 'Microsoft Entra' },
  { value: 'okta', label: 'Okta' },
  { value: 'keycloak', label: 'Keycloak' },
  { value: 'google', label: 'Google' },
  { value: 'other', label: 'Other' },
];

export function Directory() {
  const [saved, setSaved] = useState<SsoAnswer | null>(null);
  const [sso, setSso] = useState<Sso | null>(null);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);

  useEffect(() => {
    api<SsoAnswer>('GET', '/api/sso')
      .then((a) => {
        setSaved(a);
        setSso(a.sso);
      })
      .catch((e) => toast(friendly(e), 'error'));
  }, []);
  if (!saved || !sso) return null;

  const set = <K extends keyof Sso>(k: K, v: Sso[K]) => setSso({ ...sso, [k]: v });
  const dirty = JSON.stringify(sso) !== JSON.stringify(saved.sso);
  const save = async () => {
    setSaving(true);
    try {
      const a = await api<SsoAnswer>('PUT', '/api/sso', sso);
      setSaved(a);
      setSso(a.sso);
      toast(t('Settings saved'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setSaving(false);
  };
  const test = async () => {
    setTesting(true);
    try {
      const r = await api<{ issuer: string; keys: number }>('POST', '/api/sso/test', { issuer: sso.issuer });
      toast(`${t('{issuer} answers', { issuer: r.issuer })} · ${tn(r.keys, '{n} key', '{n} keys')}`);
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setTesting(false);
  };
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(saved.redirectUri);
      toast(t('Copied'));
    } catch {
      /* le champ reste sélectionnable */
    }
  };

  return (
    <>
      <PageHead title={t('Directory')} sub={saved.sso.enabled ? `OpenID Connect · ${saved.buttonName}` : t('Off')} />
      <Card icon={<KeyRound size={16} />} title="OpenID Connect">
        <Switch checked={sso.enabled} onChange={(v) => set('enabled', v)} label={t('Sign in with the directory')} />
        <FieldGrid two>
          <Field label={t('Provider')} group>
            <Dropdown value={sso.provider} onChange={(v) => set('provider', v)} options={PROVIDERS.map((p) => ({ ...p, label: p.value === 'other' ? t('Other') : p.label }))} />
          </Field>
          <Field label={t('Issuer')}>
            <input value={sso.issuer} placeholder="https://" spellCheck={false} onChange={(e) => set('issuer', e.target.value)} />
          </Field>
          <Field label={t('Client id')}>
            <input value={sso.clientId} spellCheck={false} onChange={(e) => set('clientId', e.target.value)} />
          </Field>
          <Field label={t('Client secret')}>
            <input type="password" value={sso.clientSecret} autoComplete="off" onChange={(e) => set('clientSecret', e.target.value)} />
          </Field>
        </FieldGrid>
        <Field label={t('Redirect address')} group>
          <div className="core-copy">
            <input value={saved.redirectUri} readOnly />
            <button type="button" className="st-btn" onClick={() => void copy()}>
              <Copy size={14} /> {t('Copy')}
            </button>
          </div>
        </Field>
        <div className="st-row">
          <button type="button" className="st-btn" disabled={!sso.issuer.trim() || testing} onClick={() => void test()}>
            <Send size={14} /> {t('Test')}
          </button>
        </div>
      </Card>
      <Card icon={<ShieldCheck size={16} />} title={t('Access')}>
        <FieldGrid two>
          <Field label={t('Server administrators: directory groups')} group>
            <TagInput value={sso.adminGroups} onChange={(v) => set('adminGroups', v)} placeholder="/it-admins" />
          </Field>
          <Field label={t('Allowed e-mail domains')} group>
            <TagInput value={sso.domains} onChange={(v) => set('domains', v)} placeholder="exemple.fr" />
          </Field>
          <Field label={t('Groups claim')}>
            <input value={sso.groupsClaim} spellCheck={false} onChange={(e) => set('groupsClaim', e.target.value)} />
          </Field>
          <Field label={t('Local accounts')} group>
            <Segmented<LocalAccounts>
              value={sso.localAccounts}
              onChange={(v) => set('localAccounts', v)}
              options={[
                { value: 'none', label: t('None') },
                { value: 'adminOnly', label: t('Administrators') },
                { value: 'all', label: t('All') },
              ]}
            />
          </Field>
        </FieldGrid>
        <Switch checked={sso.allowAny} onChange={(v) => set('allowAny', v)} label={t('Accept accounts in no known group')} />
      </Card>
      <SaveBar dirty={dirty} saving={saving} onSave={() => void save()} onDiscard={() => setSso(saved.sso)} />
    </>
  );
}

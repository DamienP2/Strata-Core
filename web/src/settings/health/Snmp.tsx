// Réglages › Santé › SNMP : l'agent (lecture seule, v2c) que la supervision interroge, les
// destinations des traps, la MIB, et ce que l'agent répond maintenant.
import { useEffect, useState } from 'react';
import { Download, ListTree, Network } from 'lucide-react';
import { Badge, Card, Field, FieldGrid, InfoTip, NumberField, PageHead, SaveBar, Switch, t, tn, toast } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';

interface Settings {
  enabled: boolean;
  port: number;
  community: string;
  allowed: string[];
  traps: string[];
  contact: string;
  location: string;
}

interface State {
  settings: Settings;
  listening?: string | null;
  error?: string | null;
  root: string;
  walk: { oid: string; name: string; value: string }[];
}

const lines = (v: string[]) => v.join('\n');
const split = (v: string) => v.split('\n');

export function SnmpPage() {
  const [st, setSt] = useState<State | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const read = async (first = false) => {
    try {
      const r = await api<State>('GET', '/api/health/snmp');
      setSt(r);
      if (first) setDraft(r.settings);
    } catch (e) {
      if (first) toast(friendly(e), 'error');
    }
  };
  useEffect(() => {
    void read(true);
  }, []);
  if (!st || !draft) return <PageHead title="SNMP" />;
  const dirty = JSON.stringify(draft) !== JSON.stringify(st.settings);
  const edit = (p: Partial<Settings>) => setDraft({ ...draft, ...p });
  const save = async () => {
    setSaving(true);
    try {
      const r = await api<{ settings: Settings; listening?: string | null; error?: string | null }>('PUT', '/api/health/snmp', draft);
      setSt({ ...st, ...r });
      setDraft(r.settings);
      if (r.error) toast(friendly(r.error), 'error');
      else toast(t('Settings saved'));
      void read();
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setSaving(false);
  };
  const status = st.error ? <Badge tone="bad">{t('Not listening')}</Badge> : st.listening ? <Badge tone="ok">{t('Listening on UDP {port}', { port: st.listening.split(':').pop() ?? '' })}</Badge> : <Badge>{t('Off')}</Badge>;
  return (
    <>
      <PageHead title="SNMP" sub={t('Read only, SNMP v2c')}>
        <a className="st-btn" href="/api/health/snmp/mib" download>
          <Download size={14} /> STRATA-CORE-MIB
        </a>
      </PageHead>

      <Card icon={<Network size={16} />} title={t('Agent')} actions={status}>
        <div className="core-clock-form">
          {st.error && <p className="core-warn">{friendly(st.error)}</p>}
          <Switch checked={draft.enabled} onChange={(v) => edit({ enabled: v })} label={t('Answer the supervision')} />
          {draft.enabled && (
            <>
          <FieldGrid>
            <Field label={t('Community')}>
              <input type="password" value={draft.community} autoComplete="new-password" onChange={(e) => edit({ community: e.target.value })} />
            </Field>
            <Field label={t('UDP port')} group>
              <NumberField value={draft.port} min={1} max={65535} onChange={(v) => edit({ port: Math.round(v) })} />
            </Field>
          </FieldGrid>
          <FieldGrid two>
            <Field
              label={
                <>
                  {t('Allowed machines')} <InfoTip text={t('One per line: 10.0.0.5 or 10.0.0.0/24; empty: all')} />
                </>
              }
            >
              <textarea rows={3} className="st-mono" value={lines(draft.allowed)} onChange={(e) => edit({ allowed: split(e.target.value) })} />
            </Field>
            <Field
              label={
                <>
                  {t('Trap destinations')} <InfoTip text={t('One per line: 10.0.0.9 or 10.0.0.9:162')} />
                </>
              }
            >
              <textarea rows={3} className="st-mono" value={lines(draft.traps)} onChange={(e) => edit({ traps: split(e.target.value) })} />
            </Field>
          </FieldGrid>
          <FieldGrid two>
            <Field label={t('Contact')}>
              <input value={draft.contact} placeholder="technique@exemple.fr" onChange={(e) => edit({ contact: e.target.value })} />
            </Field>
            <Field label={t('Location')}>
              <input value={draft.location} placeholder={t('Control room A, rack 3')} onChange={(e) => edit({ location: e.target.value })} />
            </Field>
          </FieldGrid>
            </>
          )}
        </div>
      </Card>

      {/* Un outil de contrôle, pas un réglage : replié. */}
      <details className="core-fold">
        <summary>
          <ListTree size={15} /> {t('What the agent answers')}{' '}
          <span className="st-muted">
            {tn(st.walk.length, '{n} value', '{n} values')} · {st.root}
          </span>
        </summary>
        <div className="core-fold-body">
          <table className="core-table">
            <tbody>
              {st.walk.map((w) => (
                <tr key={w.oid}>
                  <td className="st-mono st-muted">{w.oid}</td>
                  <td>{w.name}</td>
                  <td className="st-mono">{w.value}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </details>
      <SaveBar dirty={dirty} saving={saving} onSave={() => void save()} onDiscard={() => setDraft(st.settings)} />
    </>
  );
}

// Réglages › Santé › Alertes : les problèmes en cours, où les envoyer (webhook, e-mail), quoi
// envoyer, et les derniers messages.
import { useRef, useState } from 'react';
import { BellRing, CircleAlert, Send, Settings2 } from 'lucide-react';
import { Badge, Card, Empty, Field, FieldGrid, NumberField, PageHead, SaveBar, Segmented, SubTabs, Switch, t, timeAgo, toast } from '@strata/ui';
import { api } from '../../api';
import { navigate } from '../../router';
import { friendly } from '../../errors';
import { useJsonState, useRepeat } from '../../poll';

interface Settings {
  enabled: boolean;
  language: 'fr' | 'en';
  webhookEnabled: boolean;
  emailEnabled: boolean;
  snmpTraps: boolean;
  webhook: string;
  emailTo: string;
  smtpHost: string;
  smtpPort: number;
  smtpSecurity: 'starttls' | 'tls' | 'none';
  smtpUser: string;
  smtpPassword: string;
  smtpFrom: string;
  delaySecs: number;
  system: boolean;
  apps: boolean;
  hardware: boolean;
  clock: boolean;
  restart: boolean;
}

interface Problem {
  key: string;
  event: string;
  since: number;
  text: string;
  told: boolean;
}

interface Sent {
  at: number;
  level: 'problem' | 'ok' | 'info';
  text: string;
  results: [string, string | null][];
}

interface State {
  settings: Settings;
  active: Problem[];
  recent: Sent[];
}

const KINDS: { key: 'system' | 'apps' | 'hardware' | 'clock' | 'restart'; label: string }[] = [
  { key: 'system', label: 'Disks, memory, processors' },
  { key: 'apps', label: 'An app that no longer runs' },
  { key: 'hardware', label: 'DeckLink cards, REF, SDI inputs' },
  { key: 'clock', label: 'PTP, server time' },
  { key: 'restart', label: 'Restart after an unexpected stop' },
];

export function AlertsPage({ onCount }: { onCount?: (n: number) => void }) {
  const [st, setSt] = useJsonState<State | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [tab, setTab] = useState<'now' | 'settings'>('now');
  // La première lecture réussie donne la copie à éditer ; les erreurs de la première seules sont dites.
  const first = useRef(true);
  // `alive` : faux une fois la page quittée ; rien ne change alors.
  const read = async (alive: () => boolean) => {
    const isFirst = first.current;
    try {
      const r = await api<State>('GET', '/api/health/alerts');
      if (!alive()) return;
      first.current = false;
      setSt(r);
      onCount?.(r.active.length);
      setDraft((d) => d ?? r.settings);
    } catch (e) {
      if (!alive()) return;
      first.current = false;
      if (isFirst) toast(friendly(e), 'error');
    }
  };
  useRepeat('alerts', 5000, read);
  if (!st || !draft) return <PageHead title={t('Alerts')} />;
  const dirty = JSON.stringify(draft) !== JSON.stringify(st.settings);
  const edit = (p: Partial<Settings>) => setDraft({ ...draft, ...p });
  const save = async () => {
    setSaving(true);
    try {
      const r = await api<{ settings: Settings }>('PUT', '/api/health/alerts', draft);
      setSt((cur) => cur && { ...cur, settings: r.settings });
      setDraft(r.settings);
      toast(t('Settings saved'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setSaving(false);
  };
  const test = async () => {
    setTesting(true);
    try {
      const r = await api<{ results: [string, string | null][] }>('POST', '/api/health/alerts/test', draft);
      for (const [channel, err] of r.results) {
        const name = channel === 'webhook' ? 'Webhook' : channel === 'snmp' ? t('SNMP traps') : t('E-mail');
        toast(err ? t('{channel}: {error}', { channel: name, error: friendly(err) }) : t('{channel}: test message sent', { channel: name }), err ? 'error' : 'info');
      }
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setTesting(false);
  };

  return (
    <>
      <PageHead title={t('Alerts')} sub={st.settings.enabled ? t('Sent after {n} s', { n: st.settings.delaySecs }) : t('Not sent')} />
      <SubTabs<'now' | 'settings'>
        items={[
          { id: 'now', label: t('Now'), count: st.active.length },
          { id: 'settings', label: t('Sending') },
        ]}
        value={tab}
        onChange={setTab}
      />

      {tab === 'now' && (
        <>
      <Card icon={<CircleAlert size={16} />} title={t('Now')} actions={st.active.length ? <Badge tone="warn">{st.active.length}</Badge> : <Badge tone="ok">{t('All good')}</Badge>}>
        {st.active.length === 0 ? (
          <Empty text={t('Nothing to report')} />
        ) : (
          <div className="core-checks">
            {st.active.map((p) => (
              <div key={p.key} className="core-check warn">
                <CircleAlert size={15} />
                <span className="core-incident-title">{p.text}</span>
                <span className="st-muted core-incident-when">
                  {t('since {when}', { when: timeAgo(p.since) })}
                  {p.told ? ` · ${t('sent')}` : ''}
                </span>
              </div>
            ))}
          </div>
        )}
      </Card>

      <RecentCard recent={st.recent} />
        </>
      )}

      {tab === 'settings' && (
      <Card
        icon={<Settings2 size={16} />}
        title={t('Sending')}
        actions={
          <button type="button" className="st-btn small" disabled={testing || !(draft.webhookEnabled || draft.emailEnabled || draft.snmpTraps)} onClick={() => void test()}>
            <Send size={13} /> {t('Send a test')}
          </button>
        }
      >
        <div className="core-clock-form">
          <Switch checked={draft.enabled} onChange={(v) => edit({ enabled: v })} label={t('Send the alerts')} />
          {!draft.enabled && <p className="st-muted core-note">{t('Nothing is sent: the problems are only shown here and in the supervision (SNMP).')}</p>}
          {draft.enabled && (
            <>
          <div className="core-channel">
            <Switch checked={draft.webhookEnabled} onChange={(v) => edit({ webhookEnabled: v })} label="Webhook" />
            {draft.webhookEnabled && (
              <Field label={t('Address')} hint={t('Slack, Discord, Mattermost, JSON. Teams: in the channel, Workflows › “Send webhook alerts to a channel”, then paste its address.')}>
                <input value={draft.webhook} placeholder="https://hooks.slack.com/services/…" onChange={(e) => edit({ webhook: e.target.value })} />
              </Field>
            )}
          </div>
          <div className="core-channel">
            <Switch checked={draft.emailEnabled} onChange={(v) => edit({ emailEnabled: v })} label={t('E-mail')} />
            {draft.emailEnabled && (
              <>
                <Field label={t('Recipients')}>
                  <input value={draft.emailTo} placeholder="regie@exemple.fr, technique@exemple.fr" onChange={(e) => edit({ emailTo: e.target.value })} />
                </Field>
                <FieldGrid>
                  <Field label={t('Mail server')}>
                    <input value={draft.smtpHost} placeholder="smtp.exemple.fr" onChange={(e) => edit({ smtpHost: e.target.value })} />
                  </Field>
                  <Field label={t('Port')} group>
                    <NumberField value={draft.smtpPort} min={1} max={65535} onChange={(v) => edit({ smtpPort: Math.round(v) })} />
                  </Field>
                  <Field label={t('Security')} group>
                    <Segmented<Settings['smtpSecurity']>
                      value={draft.smtpSecurity}
                      options={[
                        { value: 'starttls', label: 'STARTTLS' },
                        { value: 'tls', label: 'TLS' },
                        { value: 'none', label: t('None') },
                      ]}
                      onChange={(v) => edit({ smtpSecurity: v })}
                    />
                  </Field>
                </FieldGrid>
                <FieldGrid>
                  <Field label={t('User')}>
                    <input value={draft.smtpUser} autoComplete="off" onChange={(e) => edit({ smtpUser: e.target.value })} />
                  </Field>
                  <Field label={t('Password')}>
                    <input type="password" value={draft.smtpPassword} autoComplete="new-password" onChange={(e) => edit({ smtpPassword: e.target.value })} />
                  </Field>
                  <Field label={t('Sender')}>
                    <input value={draft.smtpFrom} placeholder="strata@exemple.fr" onChange={(e) => edit({ smtpFrom: e.target.value })} />
                  </Field>
                </FieldGrid>
              </>
            )}
          </div>
          <div className="core-channel">
            <Switch checked={draft.snmpTraps} onChange={(v) => edit({ snmpTraps: v })} label={t('SNMP traps')} />
            {draft.snmpTraps && (
              <button type="button" className="st-btn small ghost" onClick={() => navigate('/reglages/snmp')}>
                {t('Destinations: Health › SNMP')}
              </button>
            )}
          </div>
          <FieldGrid two>
            <Field label={t('Delay before an alert (s)')} group>
              <NumberField value={draft.delaySecs} min={5} max={3600} onChange={(v) => edit({ delaySecs: Math.round(v) })} />
            </Field>
            <Field label={t('Language of the messages')} group>
              <Segmented<'fr' | 'en'>
                value={draft.language}
                options={[
                  { value: 'fr', label: 'Français' },
                  { value: 'en', label: 'English' },
                ]}
                onChange={(v) => edit({ language: v })}
              />
            </Field>
          </FieldGrid>
          <Field label={t('What is sent')} group>
            <div className="core-switches">
              {KINDS.map((k) => (
                <Switch key={k.key} checked={draft[k.key]} onChange={(v) => edit({ [k.key]: v })} label={t(k.label)} />
              ))}
            </div>
          </Field>
            </>
          )}
        </div>
      </Card>
      )}

      <SaveBar dirty={dirty} saving={saving} onSave={() => void save()} onDiscard={() => setDraft(st.settings)} />
    </>
  );
}

/** Les derniers messages envoyés (et ce qui a échoué). */
function RecentCard({ recent }: { recent: State['recent'] }) {
  return (
      <Card icon={<BellRing size={16} />} title={t('Last messages')}>
        {recent.length === 0 ? (
          <Empty text={t('No message sent yet')} />
        ) : (
          <div className="core-checks">
            {recent.map((m, i) => {
              const failed = m.results.filter(([, e]) => e);
              return (
                <div key={i} className={`core-check ${m.level === 'problem' ? 'warn' : 'ok'}`}>
                  {m.level === 'problem' ? <CircleAlert size={15} /> : <BellRing size={15} />}
                  <span className="core-incident-title">
                    {m.text}
                    {failed.map(([c, e]) => (
                      <span key={c} className="core-warn">
                        {' '}
                        · {c} : {friendly(e ?? '')}
                      </span>
                    ))}
                  </span>
                  <span className="st-muted core-incident-when">{timeAgo(m.at)}</span>
                </div>
              );
            })}
          </div>
        )}
      </Card>
  );
}

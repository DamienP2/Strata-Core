// Réglages › Ce serveur : la vue d'ensemble, le nom et les adresses, le HTTPS.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { CircleCheck, Download, FileKey, Globe, Lock, Package, RefreshCw, RotateCw, Server as ServerIcon, ShieldCheck, TriangleAlert } from 'lucide-react';
import {
  Badge, Card, Field, FieldGrid, PageHead, SaveBar, Segmented, confirmDialog, formatDate, formatDuration, setTimezone, t, tn, toast,
} from '@strata/ui';
import { api, expectRestart } from '../api';
import { friendly } from '../errors';
import { navigate } from '../router';
import type { AppInfo } from './Library';
import { serviceTone } from './Library';

type Mode = 'off' | 'strata' | 'company' | 'proxy';

interface Listening {
  port?: number | null;
  error?: string | null;
}

interface ServerInfo {
  name: string;
  dnsName: string;
  timezone: string;
  hostname: string;
  version: string;
  uptimeSecs: number;
  http: string | null;
  https: { mode: Mode; port: number; listening: Listening };
  scheme: string;
  addresses: { label: string; url: string; app?: string }[];
  apps: { installed: number; active: number };
}

interface CertInfo {
  origin: 'strata' | 'company';
  subject: string;
  issuer: string;
  names: string[];
  notBefore: number;
  notAfter: number;
  missing: string[];
}

interface HttpsInfo {
  mode: Mode;
  port: number;
  listening: Listening;
  cert: CertInfo | null;
  company: CertInfo | null;
  ca: boolean;
  names: string[];
  http: number | null;
}

function useServer() {
  const [info, setInfo] = useState<ServerInfo | null>(null);
  const reload = useCallback(async () => {
    try {
      setInfo(await api<ServerInfo>('GET', '/api/server'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { info, setInfo, reload };
}



function httpsLine(h: ServerInfo['https']) {
  if (h.mode === 'off') return <Badge>{t('HTTP only')}</Badge>;
  if (h.mode === 'proxy') return <Badge tone="ok">{t('By a relay in front of the Core')}</Badge>;
  if (h.listening.port) return <Badge tone="ok">{t('Port {port}', { port: h.listening.port })}</Badge>;
  return <Badge tone="bad">{t('Not listening')}</Badge>;
}

/** Vue d'ensemble : ce serveur, ses applis, le redémarrage du Core. */
export function Overview({ apps }: { apps: AppInfo[] | null }) {
  const { info } = useServer();
  const [restarting, setRestarting] = useState(false);
  if (!info) return null;
  const restart = async () => {
    const ok = await confirmDialog({
      title: t('Restart the Core?'),
      message: t('The cockpit, the settings and the apps started by the Core stop for a few seconds.'),
      confirm: t('Restart'),
    });
    if (!ok) return;
    setRestarting(true);
    try {
      await api('POST', '/api/server/restart');
      expectRestart('restart');
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setRestarting(false);
  };
  return (
    <>
      <PageHead title={info.name} sub={`${info.dnsName} · Core ${info.version} · ${t('running for {time}', { time: formatDuration(info.uptimeSecs) })}`}>
        <button type="button" className="st-btn" disabled={restarting} onClick={() => void restart()}>
          <RotateCw size={14} /> {restarting ? t('Restarting…') : t('Restart the Core')}
        </button>
      </PageHead>
      <div className="core-cols">
        <Card icon={<ServerIcon size={16} />} title={t('This server')}>
          <div className="core-lines">
            <div className="core-line">
              <span>{t('Machine')}</span>
              <span className="st-mono st-muted">{info.hostname}</span>
            </div>
            <div className="core-line">
              <span>{t('DNS name')}</span>
              <span className="st-mono st-muted">{info.dnsName}</span>
            </div>
            <div className="core-line">
              <span>HTTP</span>
              <span className="st-mono st-muted">{info.http ?? '—'}</span>
            </div>
            <div className="core-line">
              <span>HTTPS</span>
              {httpsLine(info.https)}
            </div>
            <div className="core-line">
              <span>{t('Time zone')}</span>
              <span className="st-muted">{info.timezone}</span>
            </div>
          </div>
        </Card>
        <Card icon={<Package size={16} />} title={t('Apps')}>
          {apps && apps.length > 0 ? (
            <div className="core-lines">
              {apps.map((a) => {
                const tone = serviceTone(a);
                return (
                  <button key={a.id} type="button" className="core-line click" onClick={() => navigate(`/reglages/applis/${a.id}`)}>
                    <span>
                      <span className={`st-dot ${tone ?? ''}`} /> {a.name}
                    </span>
                    <span className="st-muted st-mono">{a.active ? a.version : t('Inactive')}</span>
                  </button>
                );
              })}
            </div>
          ) : (
            <span className="st-muted">{t('No app installed')}</span>
          )}
        </Card>
      </div>
    </>
  );
}

interface Dns {
  name: string;
  base: string[];
  baseHere: boolean;
  wildcard: string[];
  wildcardHere: boolean;
  local: string[];
}

function DnsLine({ name, ips, here }: { name: string; ips: string[]; here: boolean }) {
  if (!ips.length)
    return (
      <div className="core-line">
        <span className="st-mono">{name}</span>
        <Badge tone="bad">{t('Not resolved')}</Badge>
      </div>
    );
  return (
    <div className="core-line">
      <span className="st-mono">{name}</span>
      <span className="core-service">
        <span className="st-mono st-muted">{ips.join(', ')}</span>
        {here ? (
          <Badge tone="ok">
            <CircleCheck size={11} /> {t('This server')}
          </Badge>
        ) : (
          <Badge tone="warn">{t('Another machine')}</Badge>
        )}
      </span>
    </div>
  );
}

/** Le nom DNS et ses sous-domaines mènent-ils ici ? Sinon, quoi créer dans le DNS. */
function DnsCheck({ dnsName }: { dnsName: string }) {
  const [dns, setDns] = useState<Dns | null>(null);
  const [busy, setBusy] = useState(false);
  const check = useCallback(async () => {
    setBusy(true);
    try {
      setDns(await api<Dns>('GET', '/api/server/dns'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setBusy(false);
  }, []);
  useEffect(() => {
    void check();
  }, [check, dnsName]);
  const ip = dns?.local.find((a) => !a.includes(':')) ?? dns?.local[0] ?? '…';
  const ok = dns && dns.baseHere && dns.wildcardHere;
  return (
    <Card icon={<Globe size={16} />} title={t('Name resolution')}>
      {dns && (
        <div className="core-lines">
          <DnsLine name={dns.name} ips={dns.base} here={dns.baseHere} />
          <DnsLine name={`*.${dns.name}`} ips={dns.wildcard} here={dns.wildcardHere} />
        </div>
      )}
      {dns && !ok && (
        <p className="st-muted core-note">
          {t('Ask for these two records in the company DNS, towards this server ({ip}):', { ip })}{' '}
          <span className="st-mono">{dns.name}</span> · <span className="st-mono">*.{dns.name}</span>
        </p>
      )}
      <div className="st-row">
        <button type="button" className="st-btn" disabled={busy} onClick={() => void check()}>
          <RefreshCw size={14} /> {t('Check again')}
        </button>
      </div>
    </Card>
  );
}

/** Le nom du serveur, son nom DNS (les adresses), son fuseau horaire ; les adresses qui en découlent. */
export function Addresses() {
  const { info, setInfo } = useServer();
  const [form, setForm] = useState<{ name: string; dnsName: string; timezone: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const zones = useMemo(() => {
    try {
      return (Intl as unknown as { supportedValuesOf(k: string): string[] }).supportedValuesOf('timeZone');
    } catch {
      return ['Europe/Paris', 'UTC'];
    }
  }, []);
  useEffect(() => {
    if (info && !form) setForm({ name: info.name, dnsName: info.dnsName, timezone: info.timezone });
  }, [info, form]);
  if (!info || !form) return null;
  const saved = { name: info.name, dnsName: info.dnsName, timezone: info.timezone };
  const dirty = JSON.stringify(form) !== JSON.stringify(saved);
  const save = async () => {
    if (form.dnsName.trim().toLowerCase() !== info.dnsName) {
      const ok = await confirmDialog({
        title: t('Change the DNS name?'),
        message: t('The addresses of the cockpit and of every app change. Bookmarks and the DNS records must follow.'),
        confirm: t('Change'),
      });
      if (!ok) return;
    }
    setSaving(true);
    try {
      const next = await api<ServerInfo>('PUT', '/api/server', form);
      setInfo(next);
      setForm({ name: next.name, dnsName: next.dnsName, timezone: next.timezone });
      setTimezone(next.timezone);
      toast(t('Settings saved'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setSaving(false);
  };
  const set = (k: keyof typeof form, v: string) => setForm({ ...form, [k]: v });
  return (
    <>
      <PageHead title={t('Name and addresses')} sub={info.dnsName} />
      <Card icon={<ServerIcon size={16} />} title={t('Server')}>
        <FieldGrid>
          <Field label={t('Name')} hint={t('What users see (“Control room A”).')}>
            <input value={form.name} onChange={(e) => set('name', e.target.value)} />
          </Field>
          <Field label={t('DNS name')} hint={t('The cockpit answers there, each app at a sub-domain.')}>
            <input value={form.dnsName} spellCheck={false} autoCapitalize="none" onChange={(e) => set('dnsName', e.target.value)} />
          </Field>
          <Field label={t('Time zone')}>
            <input value={form.timezone} list="core-zones" spellCheck={false} onChange={(e) => set('timezone', e.target.value)} />
            <datalist id="core-zones">
              {zones.map((z) => (
                <option key={z} value={z} />
              ))}
            </datalist>
          </Field>
        </FieldGrid>
      </Card>
      <DnsCheck dnsName={info.dnsName} />
      <Card icon={<Globe size={16} />} title={t('Addresses')}>
        <table className="st-table">
          <thead>
            <tr>
              <th>{t('Page')}</th>
              <th>{t('Address')}</th>
            </tr>
          </thead>
          <tbody>
            {info.addresses.map((a) => (
              <tr key={a.url}>
                <td>
                  <strong>{a.app ? a.label : t(a.label)}</strong>
                </td>
                <td>
                  <a className="core-link st-mono" href={a.url} target="_blank" rel="noreferrer noopener">
                    {a.url}
                  </a>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
      <SaveBar dirty={dirty} saving={saving} onSave={() => void save()} onDiscard={() => setForm(saved)} />
    </>
  );
}

/** Le certificat servi : ses noms, son émetteur, sa fin. */
function CertCard({ cert, title }: { cert: CertInfo; title: string }) {
  const days = Math.floor((cert.notAfter * 1000 - Date.now()) / 86400000);
  return (
    <Card icon={<ShieldCheck size={16} />} title={title}>
      <FieldGrid two>
        <Field label={t('Covered names')}>
          <input value={cert.names.join(', ')} readOnly />
        </Field>
        <Field label={t('Expires on')}>
          <input value={`${formatDate(cert.notAfter * 1000)} (${tn(days, 'in {n} day', 'in {n} days')})`} readOnly />
        </Field>
      </FieldGrid>
      <Field label={t('Issued by')}>
        <input value={cert.issuer} readOnly />
      </Field>
      {cert.missing.length > 0 && (
        <p className="core-warn">
          <TriangleAlert size={14} /> {t('Does not cover {names}: change the certificate or the DNS name.', { names: cert.missing.join(', ') })}
        </p>
      )}
    </Card>
  );
}

/** Importer le certificat de l'entreprise : sa chaîne et sa clé privée, en PEM. */
function ImportCard({ onDone }: { onDone: (h: HttpsInfo) => void }) {
  const [cert, setCert] = useState<File | null>(null);
  const [key, setKey] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  const send = async () => {
    if (!cert || !key) return;
    setBusy(true);
    try {
      const h = await api<HttpsInfo>('POST', '/api/https/import', { cert: await cert.text(), key: await key.text() });
      toast(t('Certificate imported'));
      setCert(null);
      setKey(null);
      onDone(h);
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setBusy(false);
  };
  const pick = (label: string, file: File | null, set: (f: File | null) => void) => (
    <Field label={label} group>
      <label className="core-file">
        <input type="file" accept=".pem,.crt,.cer,.key" hidden onChange={(e) => set(e.target.files?.[0] ?? null)} />
        <span className="st-btn">
          <FileKey size={14} /> {t('Choose a file')}
        </span>
        <span className="st-muted st-mono">{file ? file.name : t('No file')}</span>
      </label>
    </Field>
  );
  return (
    <Card icon={<FileKey size={16} />} title={t('Import the company certificate')}>
      <p className="st-muted core-note">{t('PEM files: the certificate with its chain (the server’s first), and its private key. They must cover the server’s name and its sub-domains.')}</p>
      <FieldGrid two>
        {pick(t('Certificate and chain'), cert, setCert)}
        {pick(t('Private key'), key, setKey)}
      </FieldGrid>
      <div className="st-row">
        <button type="button" className="st-btn primary" disabled={!cert || !key || busy} onClick={() => void send()}>
          {t('Import')}
        </button>
      </div>
    </Card>
  );
}

const MODES: { value: Mode; label: string; text: string }[] = [
  { value: 'off', label: 'None', text: 'HTTP only. For tests, or a closed network.' },
  { value: 'strata', label: 'Made by Strata', text: 'A certificate made by this server. Install its authority once on each computer: no more warnings, for the cockpit and every app.' },
  { value: 'company', label: 'Company', text: 'The company’s certificate, imported below. It must cover the server’s name and its sub-domains.' },
  { value: 'proxy', label: 'Relay', text: 'A relay in front of the Core does the HTTPS (it forwards to the Core in HTTP). The addresses start with https://.' },
];

/** Le HTTPS : son origine, son port, le certificat servi, l'autorité à installer, l'import. */
export function Https() {
  const [h, setH] = useState<HttpsInfo | null>(null);
  const [mode, setMode] = useState<Mode>('off');
  const [port, setPort] = useState('443');
  const [saving, setSaving] = useState(false);
  const take = (x: HttpsInfo) => {
    setH(x);
    setMode(x.mode);
    setPort(String(x.port));
  };
  useEffect(() => {
    api<HttpsInfo>('GET', '/api/https')
      .then(take)
      .catch((e) => toast(friendly(e), 'error'));
  }, []);
  if (!h) return null;
  const dirty = mode !== h.mode || Number(port) !== h.port;
  const served = mode === 'strata' || mode === 'company';
  const save = async () => {
    if (served && h.mode !== mode && location.protocol === 'http:') {
      const ok = await confirmDialog({
        title: t('Switch to HTTPS?'),
        message:
          mode === 'strata'
            ? t('The cockpit and the apps move to HTTPS. Install the authority of this server on this computer first (button below), or the browser will warn.')
            : t('The cockpit and the apps move to HTTPS, with the company certificate.'),
        confirm: t('Switch to HTTPS'),
      });
      if (!ok) return;
    }
    setSaving(true);
    try {
      const next = await api<HttpsInfo>('PUT', '/api/https', { mode, port: Number(port) });
      take(next);
      toast(t('Settings saved'));
      // Le HTTP renvoie maintenant vers le HTTPS : la page y va aussi.
      const p = next.listening.port;
      if (p && location.protocol === 'http:' && !['localhost', '127.0.0.1'].includes(location.hostname)) {
        location.href = `https://${location.hostname}${p === 443 ? '' : `:${p}`}${location.pathname}`;
      } else if (!p && location.protocol === 'https:' && next.mode !== 'proxy' && next.http) {
        // L'écoute HTTPS s'est arrêtée : retour en HTTP.
        location.href = `http://${location.hostname}${next.http === 80 ? '' : `:${next.http}`}${location.pathname}`;
      }
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setSaving(false);
  };
  const sub =
    h.mode === 'off'
      ? t('HTTP only')
      : h.mode === 'proxy'
        ? t('By a relay in front of the Core')
        : h.cert
          ? t('Certificate valid until {date}', { date: formatDate(h.cert.notAfter * 1000) })
          : t('No certificate');
  return (
    <>
      <PageHead title="HTTPS" sub={sub} />
      <Card icon={<Lock size={16} />} title={t('Origin')}>
        <Segmented<Mode> value={mode} onChange={setMode} options={MODES.map((m) => ({ value: m.value, label: t(m.label) }))} />
        <p className="st-muted core-note">{t(MODES.find((m) => m.value === mode)!.text)}</p>
        {served && (
          <FieldGrid two>
            <Field label={t('HTTPS port')} hint={h.http ? t('HTTP stays on port {port} and sends to HTTPS.', { port: h.http }) : undefined}>
              <input value={port} inputMode="numeric" onChange={(e) => setPort(e.target.value.replace(/\D/g, ''))} />
            </Field>
            <Field label={t('State')} group>
              <div className="core-service">
                {dirty ? (
                  <span className="st-muted">{t('Applied when saved')}</span>
                ) : h.listening.port ? (
                  <Badge tone="ok">{t('Listening on port {port}', { port: h.listening.port })}</Badge>
                ) : h.listening.error ? (
                  <Badge tone="bad">{friendly(h.listening.error)}</Badge>
                ) : (
                  <Badge>{t('Not listening')}</Badge>
                )}
              </div>
            </Field>
          </FieldGrid>
        )}
      </Card>
      {(mode === 'strata' || h.ca) && (
        <Card icon={<Download size={16} />} title={t('Authority of this server')}>
          <div className="core-service">
            <span className="st-muted">{t('To install once on each computer, among the trusted root certification authorities.')}</span>
            <div className="st-spacer" />
            {h.ca ? (
              <a className="st-btn" href="/api/https/ca.crt" download>
                <Download size={14} /> {t('Download')}
              </a>
            ) : (
              <span className="st-muted">{t('Made when Strata’s HTTPS is turned on.')}</span>
            )}
          </div>
        </Card>
      )}
      {h.cert && <CertCard cert={h.cert} title={h.cert.origin === 'strata' ? t('Certificate made by Strata') : t('Company certificate')} />}
      {mode === 'company' && h.company && h.cert?.origin !== 'company' && <CertCard cert={h.company} title={t('Company certificate')} />}
      {mode === 'company' && <ImportCard onDone={take} />}
      <SaveBar dirty={dirty} saving={saving} onSave={() => void save()} onDiscard={() => take(h)} />
    </>
  );
}

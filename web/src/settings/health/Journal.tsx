// Réglages › Santé › Journal : plantages, erreurs et avertissements du dernier jour regroupés,
// le journal du Core et des applis (30 jours), l'export pour le support.
import { useEffect, useState } from 'react';
import { Download, ListTree, Search, TriangleAlert } from 'lucide-react';
import { Badge, Card, Dropdown, Empty, PageHead, Segmented, formatDateTime, formatDuration, t, timeAgo, type DropOption } from '@strata/ui';
import { api } from '../../api';

interface Crash {
  at: number;
  kind: 'panic' | 'stop' | 'app';
  app: string;
  version: string;
  message: string;
  location: string;
  backtrace: string;
  uptimeSecs: number;
  log?: string[];
}

interface Incident {
  level: 'CRASH' | 'ERROR' | 'WARN';
  cat: string;
  app: string;
  title: string;
  detail: string;
  first: number;
  last: number;
  count: number;
  crash?: Crash;
}

interface Summary {
  version: string;
  uptimeSecs: number;
  crashes: number;
  lastCrash?: number | null;
  unseenCrash: boolean;
  errors: number;
  warnings: number;
  incidents: Incident[];
}

interface Line {
  at: number;
  level: string;
  cat: string;
  target: string;
  msg: string;
  fields?: string;
}

const CATS: Record<string, string> = { server: 'Server', hardware: 'Hardware', apps: 'Apps', users: 'People', updates: 'Updates', app: 'App' };
const LEVEL_TONE: Record<string, 'bad' | 'warn' | undefined> = { CRASH: 'bad', ERROR: 'bad', WARN: 'warn' };
const LEVEL_TEXT: Record<string, string> = { CRASH: 'Crash', ERROR: 'Error', WARN: 'Warning', INFO: 'Info' };

function crashTitle(c: Crash, names: Record<string, string>) {
  if (c.kind === 'stop') return t('The Core stopped unexpectedly (power cut, forced stop or crash)');
  if (c.kind === 'app') return t('{app} stopped by itself ({how})', { app: names[c.app] ?? c.app, how: c.message });
  return t('The Core crashed: {message}', { message: c.message });
}

function IncidentRow({ i, names }: { i: Incident; names: Record<string, string> }) {
  const [open, setOpen] = useState(false);
  const where = i.cat === 'app' ? (names[i.app] ?? i.app) : t(CATS[i.cat] ?? i.cat);
  const title = i.crash ? crashTitle(i.crash, names) : i.title;
  const more = i.crash ? i.crash.backtrace || (i.crash.log ?? []).join('\n') : '';
  return (
    <div className="core-incident">
      <button type="button" className="core-incident-head" onClick={() => more && setOpen((o) => !o)}>
        <Badge tone={LEVEL_TONE[i.level]}>{t(LEVEL_TEXT[i.level])}</Badge>
        <span className="core-incident-title">{title}</span>
        <span className="st-muted">{where}</span>
        <span className="st-muted core-incident-when">{i.count > 1 ? t('{n} times, last {when}', { n: i.count, when: timeAgo(i.last) }) : timeAgo(i.last)}</span>
      </button>
      {i.detail && <div className="st-mono st-muted core-incident-detail">{i.detail}</div>}
      {open && more && <pre className="core-pre">{more}</pre>}
    </div>
  );
}

export function JournalPage() {
  const [sum, setSum] = useState<Summary | null>(null);
  const [names, setNames] = useState<Record<string, string>>({});
  const [lines, setLines] = useState<Line[]>([]);
  const [more, setMore] = useState(false);
  const [level, setLevel] = useState<'info' | 'warn' | 'error'>('warn');
  const [cat, setCat] = useState('all');
  const [days, setDays] = useState('1');
  const [q, setQ] = useState('');

  useEffect(() => {
    api<Summary>('GET', '/api/health/summary')
      .then((s) => {
        setSum(s);
        if (s.unseenCrash) void api('POST', '/api/health/seen').catch(() => {});
      })
      .catch(() => {});
    api<{ apps: { id: string; name: string }[] }>('GET', '/api/apps')
      .then((r) => setNames(Object.fromEntries(r.apps.map((a) => [a.id, a.name]))))
      .catch(() => {});
  }, []);

  const query = (before?: number) => {
    const p = new URLSearchParams({ level, days, limit: '200' });
    if (cat !== 'all') p.set('cat', cat);
    if (q.trim()) p.set('q', q.trim());
    if (before) p.set('before', String(before));
    return `/api/health/journal?${p}`;
  };
  useEffect(() => {
    const id = setTimeout(() => {
      api<{ lines: Line[]; more: boolean }>('GET', query())
        .then((r) => {
          setLines(r.lines);
          setMore(r.more);
        })
        .catch(() => {});
    }, 250);
    return () => clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [level, cat, days, q]);
  const loadMore = async () => {
    const last = lines[lines.length - 1];
    if (!last) return;
    const r = await api<{ lines: Line[]; more: boolean }>('GET', query(last.at));
    setLines((l) => [...l, ...r.lines]);
    setMore(r.more);
  };

  const catOptions: DropOption<string>[] = [
    { value: 'all', label: t('Everything') },
    ...['server', 'hardware', 'apps', 'users', 'updates'].map((c) => ({ value: c, label: t(CATS[c]) })),
    ...Object.entries(names).map(([id, name]) => ({ value: `app:${id}`, label: name })),
  ];
  const where = (l: Line) => (l.cat === 'app' ? (names[l.target] ?? l.target) : t(CATS[l.cat] ?? l.cat));

  return (
    <>
      <PageHead title={t('Journal')} sub={sum ? `Core ${sum.version} · ${t('running for {time}', { time: formatDuration(sum.uptimeSecs) })}` : undefined}>
        <a className="st-btn" href="/api/health/export" download>
          <Download size={14} /> {t('Export for the support')}
        </a>
      </PageHead>

      {sum && (
        <div className="core-tiles">
          <div className="core-tile">
            <span>{t('Crashes (30 days)')}</span>
            <strong>{sum.crashes}</strong>
            <span className="st-muted">{sum.lastCrash ? t('last {when}', { when: timeAgo(sum.lastCrash) }) : t('none')}</span>
          </div>
          <div className="core-tile">
            <span>{t('Errors (24 h)')}</span>
            <strong>{sum.errors}</strong>
          </div>
          <div className="core-tile">
            <span>{t('Warnings (24 h)')}</span>
            <strong>{sum.warnings}</strong>
          </div>
        </div>
      )}

      <Card icon={<TriangleAlert size={16} />} title={t('Incidents')}>
        {!sum || sum.incidents.length === 0 ? (
          <Empty text={t('Nothing to report')} />
        ) : (
          <div className="core-incidents">
            {sum.incidents.map((i, k) => (
              <IncidentRow key={k} i={i} names={names} />
            ))}
          </div>
        )}
      </Card>

      <Card icon={<ListTree size={16} />} title={t('Journal')}>
        <div className="core-filters">
          <Segmented<'info' | 'warn' | 'error'>
            value={level}
            options={[
              { value: 'error', label: t('Errors') },
              { value: 'warn', label: t('Warnings') },
              { value: 'info', label: t('Everything') },
            ]}
            onChange={setLevel}
          />
          <Dropdown className="core-filter-drop" value={cat} options={catOptions} onChange={setCat} />
          <Segmented<string>
            value={days}
            options={[
              { value: '1', label: t('24 h') },
              { value: '7', label: t('7 days') },
              { value: '30', label: t('30 days') },
            ]}
            onChange={setDays}
          />
          <label className="core-search">
            <Search size={14} />
            <input value={q} placeholder={t('Search')} onChange={(e) => setQ(e.target.value)} />
          </label>
        </div>
        {lines.length === 0 ? (
          <Empty text={t('No line')} />
        ) : (
          <div className="core-journal">
            {lines.map((l, i) => (
              <div key={i} className={`core-jline ${l.level.toLowerCase()}`}>
                <span className="st-mono st-muted">{formatDateTime(l.at)}</span>
                <span className="core-jlevel">{t(LEVEL_TEXT[l.level] ?? l.level)}</span>
                <span className="st-muted">{where(l)}</span>
                <span className="core-jmsg">
                  {l.msg}
                  {l.fields && <span className="st-muted st-mono"> {l.fields}</span>}
                </span>
              </div>
            ))}
          </div>
        )}
        {more && (
          <button type="button" className="st-btn small" onClick={() => void loadMore()}>
            {t('Older lines')}
          </button>
        )}
      </Card>
    </>
  );
}

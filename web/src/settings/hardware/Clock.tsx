// Réglages › Matériel › Horloge : le timecode maison en direct, le bilan (l'heure suit-elle le PTP,
// les REF sont-elles verrouillées, le timecode des entrées est-il celui de la maison), l'état de PTP
// et NTP, les réglages, et les fichiers que le Core écrit pour le système.
import { useEffect, useRef, useState } from 'react';
import { CheckCircle2, Clock, FileCode2, Info, ListChecks, RefreshCw, Timer, TriangleAlert, XCircle } from 'lucide-react';
import { Badge, Card, Field, FieldGrid, NumberField, PageHead, Segmented, Switch, formatDateTime, t, toast } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';
import { HwSaveBar, useHardware } from './common';
import { editHw, type ApplyStep, type HouseRate } from './store';

interface Check {
  code: string;
  level: 'ok' | 'info' | 'warn' | 'bad';
  params: Record<string, string | number | boolean | null>;
}

interface ClockStatus {
  ntp: { tool?: string | null; synced?: boolean | null; enabled?: boolean | null; source?: string | null; offsetMs?: number | null };
  ptp: { running: boolean; message?: string | null; domain: number; portState?: string | null; grandmaster?: string | null; offsetNs?: number | null; jitterNs?: number | null; locked: boolean; setsClock: boolean };
  checkedAt: number;
  house: { timecode: string; frame: number; unixMs: number; fps: number; nominal: number; dropFrame: boolean; utcOffset: number };
  timezone: string;
  checks: Check[];
  files: { chrony: string; ptp4l?: string | null; ptp4lUnit?: string | null; ptpInterface?: string | null; phc?: string | null };
}

// ---------- Le timecode maison, en direct ----------

/** L'image `frame` de la journée en timecode (drop frame : 29,97 seulement). */
function tcOf(frame: number, nominal: number, df: boolean): string {
  const drop = df && nominal === 30;
  const day = drop ? 2589408 : 86400 * nominal;
  let f = ((frame % day) + day) % day;
  if (drop) {
    const d = Math.floor(f / 17982);
    const m = f % 17982;
    f += 18 * d + (m > 2 ? 2 * Math.floor((m - 2) / 1798) : 0);
  }
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(Math.floor(f / (3600 * nominal)))}:${p(Math.floor(f / (60 * nominal)) % 60)}:${p(Math.floor(f / nominal) % 60)}${drop ? ';' : ':'}${p(f % nominal)}`;
}

/** Le timecode maison qui avance de lui-même entre deux lectures du serveur. */
function LiveTc({ house }: { house: ClockStatus['house'] }) {
  const el = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const at = performance.now();
    let id = 0;
    const tick = () => {
      const n = Math.floor(((performance.now() - at) / 1000) * house.fps);
      if (el.current) el.current.textContent = tcOf(house.frame + n, house.nominal, house.dropFrame);
      id = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(id);
  }, [house]);
  return <span ref={el} className="core-clock-tc" />;
}

const RATES: { value: HouseRate; label: string }[] = [
  { value: '25', label: '25 (50 Hz)' },
  { value: '29.97', label: '29,97 (59,94 Hz)' },
  { value: '30', label: '30 (60 Hz)' },
  { value: '24', label: '24' },
  { value: '23.98', label: '23,98' },
];

function utcText(sec: number) {
  if (sec === 0) return 'UTC';
  const a = Math.abs(sec);
  const h = Math.floor(a / 3600);
  const m = Math.floor((a % 3600) / 60);
  return `UTC${sec > 0 ? '+' : '−'}${h}${m ? `:${String(m).padStart(2, '0')}` : ''}`;
}

// ---------- Le bilan ----------

const CHECK_TEXT: Record<string, string> = {
  timeFromPtp: 'The server time follows PTP (grandmaster {grandmaster})',
  ptpNotFollowed: 'PTP is locked, but the server time does not follow it yet: apply the settings',
  ptpFallbackNtp: 'PTP is not locked: the server time comes from NTP ({server}) meanwhile',
  noTime: 'The server time is not synchronised',
  timeFromNtp: 'The server time follows NTP ({server})',
  ptpNoInterface: 'No network card has the PTP role (Hardware › Network)',
  ptpNoPhc: '{interface} has no hardware clock: PTP cannot keep the time',
  twoClockSetters: 'phc2sys and chrony both set the clock: stop phc2sys',
  refLocked: '{card}: REF locked ({format}), outputs genlocked',
  refLockedNoGenlock: '{card}: REF locked ({format}), genlock off',
  refWrongRate: '{card}: the REF is {format}, not at the house rate ({house})',
  refMissing: '{card}: no REF, the outputs are not genlocked',
  refMissingNoGenlock: '{card}: no REF (genlock off)',
  inputWrongRate: '{input}: {format} is not at the house rate ({house})',
  inputTcMatches: '{input}: its timecode is the house one',
  inputTcOff: '{input}: its timecode is {frames} frames off the house one',
  inputNoTc: '{input}: no timecode in the signal',
};

function checkText(c: Check): string {
  const p: Record<string, string> = {};
  for (const [k, v] of Object.entries(c.params)) p[k] = v == null ? '—' : String(v);
  if (c.code === 'inputTcOff') p.frames = `${Number(c.params.frames) > 0 ? '+' : ''}${c.params.frames}`;
  if (c.code === 'ptpFallbackNtp' && !c.params.running) return t('ptp4l does not run: the server time comes from NTP ({server})', p);
  return t(CHECK_TEXT[c.code] ?? c.code, p);
}

const LEVEL_ICON = { ok: CheckCircle2, info: Info, warn: TriangleAlert, bad: XCircle };

function Checks({ checks }: { checks: Check[] }) {
  const worst = checks.some((c) => c.level === 'bad') ? 'bad' : checks.some((c) => c.level === 'warn') ? 'warn' : 'ok';
  return (
    <Card
      icon={<ListChecks size={16} />}
      title={t('Check-up')}
      actions={<Badge tone={worst === 'ok' ? 'ok' : worst}>{worst === 'ok' ? t('All good') : worst === 'warn' ? t('To look at') : t('To fix')}</Badge>}
    >
      <div className="core-checks">
        {checks.map((c, i) => {
          const Icon = LEVEL_ICON[c.level];
          return (
            <div key={i} className={`core-check ${c.level}`}>
              <Icon size={15} />
              <span>{checkText(c)}</span>
            </div>
          );
        })}
      </div>
    </Card>
  );
}

// ---------- Les fichiers du système ----------

const STEP_TEXT: Record<ApplyStep['what'], string> = {
  chrony: 'chrony settings written',
  ptp4l: 'ptp4l settings written',
  unit: 'strata-ptp4l service written',
  restartChrony: 'chrony restarted',
  startPtp4l: 'ptp4l started',
  stopPtp4l: 'ptp4l stopped',
};

function SystemFiles({ files }: { files: ClockStatus['files'] }) {
  const [busy, setBusy] = useState(false);
  const [steps, setSteps] = useState<ApplyStep[] | null>(null);
  const apply = async () => {
    setBusy(true);
    try {
      const r = await api<{ steps: ApplyStep[] }>('POST', '/api/hardware/clock/apply');
      setSteps(r.steps);
      toast(r.steps.every((s) => s.ok) ? t('The server time is set up') : t('The server time could not be set up completely'), r.steps.every((s) => s.ok) ? 'info' : 'error');
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setBusy(false);
  };
  return (
    <Card
      icon={<FileCode2 size={16} />}
      title={t('System files')}
      actions={
        <button type="button" className="st-btn small" disabled={busy} onClick={() => void apply()}>
          <RefreshCw size={13} /> {t('Apply again')}
        </button>
      }
    >
      <p className="st-muted core-note">{t('Written by the Core when the settings are saved; chrony and ptp4l are restarted when they change.')}</p>
      {steps && (
        <div className="core-checks">
          {steps.map((s, i) => {
            const Icon = s.ok ? CheckCircle2 : XCircle;
            return (
              <div key={i} className={`core-check ${s.ok ? 'ok' : 'bad'}`}>
                <Icon size={15} />
                <span>{s.ok ? t(STEP_TEXT[s.what]) : `${t(STEP_TEXT[s.what])} : ${friendly(s.message ?? '')}`}</span>
              </div>
            );
          })}
        </div>
      )}
      <details className="core-files">
        <summary>/etc/chrony/conf.d/strata.conf</summary>
        <pre>{files.chrony}</pre>
      </details>
      {files.ptp4l && (
        <details className="core-files">
          <summary>/etc/linuxptp/strata-ptp4l.conf</summary>
          <pre>{files.ptp4l}</pre>
        </details>
      )}
      {files.ptp4lUnit && (
        <details className="core-files">
          <summary>/etc/systemd/system/strata-ptp4l.service</summary>
          <pre>{files.ptp4lUnit}</pre>
        </details>
      )}
    </Card>
  );
}

// ---------- La page ----------

export function ClockPage() {
  const hw = useHardware();
  const [st, setSt] = useState<ClockStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const read = async () => {
    setBusy(true);
    try {
      setSt(await api<ClockStatus>('GET', '/api/hardware/clock'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setBusy(false);
  };
  const saved = hw ? JSON.stringify(hw[0].config) : '';
  useEffect(() => {
    void read();
    const id = setInterval(() => void read(), 10000);
    return () => clearInterval(id);
  }, [saved]);
  if (!hw) return null;
  const [snap, draft] = hw;
  const ptp = st?.ptp;
  const ntp = st?.ntp;
  const c = draft.clock;
  const ntsc = c.house.rate === '29.97' || c.house.rate === '23.98';
  const ptpNic = snap.nics.find((n) => n.name === draft.network.ptp);
  const ptpBadge = !ptp ? null : ptp.locked ? <Badge tone="ok">{t('Locked')}</Badge> : ptp.running ? <Badge tone="warn">{t('Not locked')}</Badge> : <Badge>{t('Not running')}</Badge>;
  const line = (label: string, value: React.ReactNode) => (
    <div className="core-line">
      <span>{label}</span>
      <span className="st-muted st-mono">{value ?? '—'}</span>
    </div>
  );
  const from = ntp?.source === 'PTP' ? t('time from PTP') : ntp?.synced ? t('time from NTP ({server})', { server: ntp.source ?? '?' }) : t('time not synchronised');
  const offsetText =
    c.house.offset === 'utc' ? 'UTC' : c.house.offset === 'fixed' ? (st ? utcText(st.house.utcOffset) : '') : st ? t('local time ({zone}, {utc})', { zone: st.timezone, utc: utcText(st.house.utcOffset) }) : '';
  return (
    <>
      <PageHead title={t('Clock')} sub={ptp?.locked && ntp?.source === 'PTP' ? t('PTP locked') : ntp?.synced ? t('NTP synchronised') : t('Not synchronised')}>
        <button type="button" className="st-btn" disabled={busy} onClick={() => void read()}>
          <RefreshCw size={14} /> {t('Read again')}
        </button>
      </PageHead>

      <Card icon={<Timer size={16} />} title={t('House timecode')}>
        <div className="core-clock">
          {st ? <LiveTc house={st.house} /> : <span className="core-clock-tc">--:--:--:--</span>}
          <div className="core-clock-sub">
            <span>{t('House rate')} {RATES.find((r) => r.value === (snap.config.clock.house.rate ?? '25'))?.label}</span>
            <span>{offsetText}</span>
            <span className={ntp?.source === 'PTP' ? 'ok' : ntp?.synced ? '' : 'bad'}>{from}</span>
          </div>
          <p className="st-muted core-note">{t('Computed from the server time, the same on every Strata server kept by the same PTP: the apps read it, no LTC cable needed.')}</p>
        </div>
      </Card>

      {st && st.checks.length > 0 && <Checks checks={st.checks} />}

      <div className="core-cols">
        <Card icon={<Clock size={16} />} title="PTP" actions={ptpBadge}>
          {ptp && !ptp.running && <p className="st-muted core-note">{t('ptp4l does not answer (is linuxptp installed and running?)')}</p>}
          <div className="core-lines">
            {line(t('Interface'), draft.network.ptp ? `${draft.network.ptp}${ptpNic?.phc ? ` · /dev/${ptpNic.phc}` : ''}` : null)}
            {line(t('Domain'), ptp?.domain)}
            {line(t('Port state'), ptp?.portState)}
            {line(t('Grandmaster'), ptp?.grandmaster)}
            {line(t('Offset'), ptp?.offsetNs != null ? `${Math.round(ptp.offsetNs)} ns` : null)}
            {line(t('Jitter'), ptp?.jitterNs != null ? `± ${Math.round(ptp.jitterNs)} ns` : null)}
          </div>
        </Card>
        <Card icon={<Clock size={16} />} title={t('Server time')} actions={ntp?.synced == null ? null : ntp.synced ? <Badge tone="ok">{t('Synchronised')}</Badge> : <Badge tone="warn">{t('Not synchronised')}</Badge>}>
          <div className="core-lines">
            {line(t('Follows'), ntp?.source === 'PTP' ? t('PTP (network card clock)') : ntp?.source)}
            {line(t('Offset'), ntp?.offsetMs != null ? (Math.abs(ntp.offsetMs) < 0.01 ? `${Math.round(ntp.offsetMs * 1e6)} ns` : `${ntp.offsetMs.toFixed(3)} ms`) : null)}
            {line(t('Read with'), ntp?.tool)}
            {line(t('Read at'), st ? formatDateTime(st.checkedAt) : null)}
          </div>
        </Card>
      </div>

      <Card icon={<Clock size={16} />} title={t('Settings')}>
        <div className="core-clock-form">
          <Field label={t('House rate')} group hint={t('The rate of every picture of the site: REF, inputs, ST 2110 flows, projects.')}>
            <Segmented<HouseRate> value={c.house.rate} options={RATES} onChange={(v) => editHw((x) => void (x.clock.house.rate = v))} />
          </Field>
          <Field label={t('Server time')} group>
            <Segmented<'ptp' | 'ntp'>
              value={c.source}
              options={[
                { value: 'ptp', label: t('PTP, NTP as a backup') },
                { value: 'ntp', label: t('NTP only') },
              ]}
              onChange={(v) => editHw((x) => void (x.clock.source = v))}
            />
          </Field>
          <Field label={t('NTP servers')} hint={t('One per line, the best first. The leader often serves NTP too.')}>
            <textarea
              rows={3}
              className="st-mono"
              value={c.ntpServers.join('\n')}
              placeholder="10.0.0.1"
              onChange={(e) => editHw((x) => void (x.clock.ntpServers = e.target.value.split('\n')))}
            />
          </Field>
          {c.source === 'ptp' && (
            <FieldGrid two>
              <Field label={t('PTP domain')} group>
                <NumberField value={draft.ptp.domain} min={0} max={127} onChange={(v) => editHw((x) => void (x.ptp.domain = Math.round(v)))} />
              </Field>
              <Field label={t('TAI − UTC (seconds)')} group>
                <NumberField value={draft.ptp.taiOffset} min={0} max={100} onChange={(v) => editHw((x) => void (x.ptp.taiOffset = Math.round(v)))} />
              </Field>
            </FieldGrid>
          )}
          <Field label={t('Timecode time')} group hint={t('Set it like the LTC of the leader.')}>
            <div className="core-service">
              <Segmented<'local' | 'utc' | 'fixed'>
                value={c.house.offset}
                options={[
                  { value: 'local', label: t('Local time') },
                  { value: 'utc', label: 'UTC' },
                  { value: 'fixed', label: t('Fixed offset') },
                ]}
                onChange={(v) => editHw((x) => void (x.clock.house.offset = v))}
              />
              {c.house.offset === 'fixed' && (
                <span className="core-service">
                  <NumberField value={c.house.fixedMinutes} min={-1440} max={1440} onChange={(v) => editHw((x) => void (x.clock.house.fixedMinutes = Math.round(v)))} />
                  <span className="st-muted">{t('minutes from UTC')}</span>
                </span>
              )}
            </div>
          </Field>
          {ntsc && (
            <FieldGrid two>
              {c.house.rate === '29.97' && (
                <Field label={t('Drop frame')} group>
                  <Switch checked={c.house.dropFrame} onChange={(v) => editHw((x) => void (x.clock.house.dropFrame = v))} label={t('Drop frame (29.97 DF)')} />
                </Field>
              )}
              <Field label={t('Daily jam')} hint={t('Each day at this time the timecode is set back to the time of day (SMPTE ST 2059-1).')}>
                <input className="st-mono" value={c.house.dailyJam} placeholder="00:00" onChange={(e) => editHw((x) => void (x.clock.house.dailyJam = e.target.value))} />
              </Field>
            </FieldGrid>
          )}
          <p className="st-muted core-note">{t('SMPTE ST 2059-2: domain 127. TAI − UTC is 37 s since 2017.')}</p>
        </div>
      </Card>

      {st && <SystemFiles files={st.files} />}
      <HwSaveBar />
    </>
  );
}

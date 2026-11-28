// Réglages › Santé › Mires et analyse. Générateur · envoyé : des mires (image, son, timecode)
// envoyées par le Core vers une sortie SDI, un flux ST 2110, une destination NDI, un flux réseau ou
// un flux MXL pour les applis. Analyseur · reçu : vérifier n'importe quelle source.
import { lazy, Suspense, useRef, useState } from 'react';
import { Pencil, Plus, Trash2, TvMinimal } from 'lucide-react';
import { Badge, Dropdown, Empty, Field, FieldGrid, Modal, PageHead, Segmented, SubTabs, Switch, confirmDialog, t, toast } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';
import { usePoll } from '../../poll';
import { Preview, Tile } from '../hardware/Inputs';

const Analyzer = lazy(() => import('./Analyzer').then((m) => ({ default: m.Analyzer })));

export interface Generator {
  id: string;
  name: string;
  enabled: boolean;
  target: string;
  format: string;
  pattern: string;
  color: string;
  ident: string;
  burnTc: boolean;
  clock: boolean;
  marker: boolean;
  tone: string;
  level: number;
  channels: number;
  channelIdent: boolean;
  tonePerChannel: boolean;
  atc: boolean;
}

interface GenStatus {
  state: 'starting' | 'running' | 'error';
  format?: string | null;
  frames: number;
  error?: string | null;
}

interface Target {
  target: string;
  label: string;
  kind: 'sdi' | '2110' | 'ndi' | 'stream' | 'mxl';
  format?: string | null;
  channels?: number | null;
}

interface View {
  generators: (Generator & { status?: GenStatus | null; flow?: string | null })[];
  targets: Target[];
  house: string;
  mxl: boolean;
}

const enc = encodeURIComponent;
const PATTERNS: [string, string][] = [
  ['bars75', 'Bars 75 %'],
  ['bars100', 'Bars 100 %'],
  ['smpte', 'SMPTE RP 219'],
  ['ramp', 'Ramp'],
  ['checker', 'Checkerboard'],
  ['zoneplate', 'Zone plate'],
  ['solid', 'Solid colour'],
  ['black', 'Black'],
  ['lipsync', 'Lip sync'],
  ['avsync', 'AV sync (per channel)'],
];
const TONES: [string, string][] = [
  ['1k', '1 kHz'],
  ['400', '400 Hz'],
  ['pink', 'Pink noise'],
  ['silence', 'Silence'],
];
const FORMATS = ['1080i50', '1080p25', '1080p50', '720p50', '1080i59.94', '1080p29.97', '1080p59.94', '720p59.94', '1080p23.98', '1080p24', '2160p23.98', '2160p24', '2160p25', '2160p29.97', '2160p50', '2160p59.94', '576i50'];
const KIND_LABEL: Record<Target['kind'], string> = { sdi: 'SDI output', '2110': 'ST 2110 flow', ndi: 'NDI destination', stream: 'Network stream', mxl: 'MXL flow' };

const patternName = (p: string) => t(PATTERNS.find(([k]) => k === p)?.[1] ?? p);
const toneName = (p: string) => t(TONES.find(([k]) => k === p)?.[1] ?? p);

/** Un identifiant d'après le nom : « Mire régie » → « mire-regie ». */
function slug(name: string, taken: string[]): string {
  const base =
    name
      .normalize('NFD')
      .replace(/[̀-ͯ]/g, '')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '')
      .slice(0, 40) || 'mire';
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
  return id;
}

export function TestPatternsPage() {
  const [tab, setTab] = useState<'generator' | 'analyzer'>('generator');
  const data = usePoll<View>('/api/testgen', 2000);
  const latest = useRef<View | null>(null);
  latest.current = data;
  const [editing, setEditing] = useState<Generator | null>(null);
  const [saving, setSaving] = useState(false);
  const save = async (generators: Generator[]) => {
    setSaving(true);
    try {
      await api('PUT', '/api/testgen', { generators: generators.map(({ status: _s, flow: _f, ...g }: Generator & { status?: unknown; flow?: unknown }) => g) });
      return true;
    } catch (e) {
      toast(friendly(e), 'error');
      return false;
    } finally {
      setSaving(false);
    }
  };
  const gens = data?.generators ?? [];
  const targetOf = (g: Generator) => (g.target === 'mxl' ? { label: `testgen/${g.id}`, kind: 'mxl' as const } : data?.targets.find((x) => x.target === g.target));
  const fresh: Generator = { id: '', name: '', enabled: true, target: 'mxl', format: '', pattern: 'bars75', color: '#336699', ident: '', burnTc: true, clock: false, marker: true, tone: '1k', level: -18, channels: 8, channelIdent: false, tonePerChannel: false, atc: true };

  return (
    <>
      <PageHead title={t('Test patterns and analysis')} sub={t('Send test signals to any output, and check any source.')} />
      <SubTabs<'generator' | 'analyzer'>
        items={[
          { id: 'generator', label: t('Generator · sent'), count: gens.length },
          { id: 'analyzer', label: t('Analyzer · received') },
        ]}
        value={tab}
        onChange={setTab}
      >
        {tab === 'generator' && (
          <button type="button" className="st-btn primary" onClick={() => setEditing({ ...fresh, name: '' })}>
            <Plus size={14} /> {t('New generator')}
          </button>
        )}
      </SubTabs>
      {tab === 'generator' && data && !data.mxl && <p className="core-warn">{t('MXL is not available: the test patterns need it.')}</p>}
      {tab === 'generator' &&
        (gens.length === 0 ? (
          <Empty icon={TvMinimal} text={t('No test pattern generator')} />
        ) : (
          <div className="core-inputs">
            {gens.map((g) => {
              const st = g.status;
              const tg = targetOf(g);
              const sub = [tg ? `${t(KIND_LABEL[tg.kind])} · ${tg.label}` : t('Destination gone'), st?.format].filter(Boolean).join(' · ');
              return (
                <Tile
                  key={g.id}
                  pic={<Preview src={`/api/testgen/${enc(g.id)}/preview.jpg`} on={st?.state === 'running'} empty={g.enabled ? t('No picture') : t('Off')} />}
                  head={
                    <>
                      <strong className="core-input-wire">{g.name}</strong>
                      {!g.enabled ? <Badge>{t('Off')}</Badge> : st?.state === 'running' ? <Badge tone="ok">{t('On air')}</Badge> : st?.state === 'error' ? <Badge tone="bad">{t('Error')}</Badge> : <Badge>{t('Starting…')}</Badge>}
                    </>
                  }
                  sub={sub}
                  error={g.enabled ? st?.error : null}
                  foot={
                    <div className="core-stream-foot">
                      <span className="st-muted">
                        {patternName(g.pattern)}
                        {g.ident ? ` · « ${g.ident} »` : ''} · {g.tone === 'silence' ? t('Silence') : `${toneName(g.tone)} ${g.level} dBFS`}
                      </span>
                      <span className="core-stream-actions">
                        <Switch
                          checked={g.enabled}
                          onChange={(on) => {
                            const cur = latest.current?.generators ?? [];
                            void save(cur.map((x) => (x.id === g.id ? { ...x, enabled: on } : x)));
                          }}
                        />
                        <button type="button" className="st-icon-btn ghost" title={t('Edit')} onClick={() => setEditing(g)}>
                          <Pencil size={14} />
                        </button>
                        <button
                          type="button"
                          className="st-icon-btn ghost"
                          title={t('Delete')}
                          onClick={async () => {
                            if (await confirmDialog({ title: t('Delete {name}?', { name: g.name }), confirm: t('Delete'), danger: true })) {
                              const cur = latest.current?.generators ?? [];
                              await save(cur.filter((x) => x.id !== g.id));
                            }
                          }}
                        >
                          <Trash2 size={14} />
                        </button>
                      </span>
                    </div>
                  }
                />
              );
            })}
          </div>
        ))}
      {tab === 'analyzer' && (
        <Suspense fallback={null}>
          <Analyzer />
        </Suspense>
      )}
      {editing && data && (
        <Editor
          value={editing}
          view={data}
          busy={saving}
          onClose={() => setEditing(null)}
          onSave={async (v) => {
            const cur = latest.current?.generators ?? [];
            const list = [...cur];
            const item = { ...v, id: v.id || slug(v.name, cur.map((x) => x.id)) };
            const k = editing.id ? list.findIndex((x) => x.id === editing.id) : -1;
            if (k < 0) list.push(item);
            else list[k] = item;
            if (await save(list)) setEditing(null);
          }}
        />
      )}
    </>
  );
}

function Editor({ value, view, busy, onClose, onSave }: { value: Generator; view: View; busy: boolean; onClose: () => void; onSave: (v: Generator) => void }) {
  const [v, setV] = useState<Generator>(value);
  const set = <K extends keyof Generator>(k: K, x: Generator[K]) => setV({ ...v, [k]: x });
  const target = view.targets.find((x) => x.target === v.target);
  const forcedFormat = target?.format ?? null;
  const forcedChannels = target?.channels ?? null;
  const taken = new Set(view.generators.filter((g) => g.enabled && g.id !== value.id && g.target !== 'mxl').map((g) => g.target));
  const valid = v.name.trim().length > 0;
  return (
    <Modal
      title={value.name ? t('Edit {name}', { name: value.name }) : t('New generator')}
      icon={<TvMinimal size={16} />}
      wide
      onClose={onClose}
      onSubmit={() => valid && onSave({ ...v, name: v.name.trim() })}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={busy || !valid}>
            {t('OK')}
          </button>
        </>
      }
    >
      <FieldGrid two>
        <Field label={t('Name')}>
          <input value={v.name} placeholder={t('Control room test pattern')} onChange={(e) => set('name', e.target.value)} />
        </Field>
        <Field label={t('Where it goes')} group>
          <Dropdown
            value={v.target}
            options={[
              { value: 'mxl', label: t('MXL flow for the apps') },
              ...view.targets.map((x) => ({ value: x.target, label: `${t(KIND_LABEL[x.kind])} · ${x.label}`, disabled: taken.has(x.target) })),
            ]}
            onChange={(x) => set('target', x)}
          />
        </Field>
        <Field label={t('Format')} hint={forcedFormat ? t('Set by the ST 2110 flow.') : undefined} group>
          {forcedFormat ? (
            <input value={forcedFormat} disabled />
          ) : (
            <Dropdown value={v.format} options={[{ value: '', label: t('House rate ({format})', { format: view.house }) }, ...FORMATS.map((f) => ({ value: f, label: f }))]} onChange={(x) => set('format', x)} />
          )}
        </Field>
        <Field label={t('Identification')} hint={t('Shown on the picture. {name} and {format} are replaced. Empty: none.')}>
          <input value={v.ident} maxLength={48} placeholder="{name} · {format}" onChange={(e) => set('ident', e.target.value)} />
        </Field>
      </FieldGrid>
      <Field label={t('Picture')} group>
        <div className="core-chips">
          {PATTERNS.map(([k, label]) => (
            <button key={k} type="button" className={`core-chip${v.pattern === k ? ' on' : ''}`} aria-pressed={v.pattern === k} onClick={() => set('pattern', k)}>
              {t(label)}
            </button>
          ))}
        </div>
      </Field>
      {v.pattern === 'solid' && (
        <Field label={t('Colour')}>
          <input type="color" value={v.color} onChange={(e) => set('color', e.target.value)} />
        </Field>
      )}
      {v.pattern === 'lipsync' && <p className="st-muted core-note">{t('A white flash and a cut in the tone once a second, at the same instant: the analyzer measures the lip sync with it.')}</p>}
      {v.pattern === 'avsync' && <p className="st-muted core-note">{t('A cursor sweeps a ±500 ms scale and reaches 0 at the white flash, with a short beep on every channel at the same instant: by eye, the cursor shows where the beep is heard; the analyzer measures each channel.')}</p>}
      <div className="core-switches">
        <Switch checked={v.burnTc} onChange={(x) => set('burnTc', x)} label={t('House timecode burnt in')} />
        <Switch checked={v.clock} onChange={(x) => set('clock', x)} label={t('Server time')} />
        <Switch checked={v.marker} onChange={(x) => set('marker', x)} label={t('Moving marker (a frozen picture shows)')} />
      </div>
      <Field label={t('Sound')} group>
        <Segmented value={v.tone} options={TONES.map(([k, label]) => ({ value: k, label: t(label) }))} onChange={(x) => set('tone', x)} />
      </Field>
      <FieldGrid two>
        <Field label={t('Level')} group>
          <Dropdown
            value={String(v.level)}
            options={[
              { value: '-18', label: '−18 dBFS (EBU)' },
              { value: '-20', label: '−20 dBFS (SMPTE)' },
              { value: '-12', label: '−12 dBFS' },
              { value: '0', label: '0 dBFS' },
            ]}
            onChange={(x) => set('level', Number(x))}
          />
        </Field>
        <Field label={t('Channels')} hint={forcedChannels ? t('Set by the ST 2110 audio flows.') : undefined} group>
          {forcedChannels ? (
            <input value={forcedChannels} disabled />
          ) : (
            <Dropdown value={String(v.channels)} options={[2, 4, 8, 16].map((n) => ({ value: String(n), label: String(n) }))} onChange={(x) => set('channels', Number(x))} />
          )}
        </Field>
      </FieldGrid>
      <div className="core-switches">
        <Switch checked={v.channelIdent} onChange={(x) => set('channelIdent', x)} label={t('Channel identification (channel n cut n times every 4 s)')} />
        <Switch checked={v.tonePerChannel} onChange={(x) => set('tonePerChannel', x)} label={t('One frequency per channel (channel n at n times the tone)')} />
        {target?.kind === '2110' && <Switch checked={v.atc} onChange={(x) => set('atc', x)} label={t('House timecode in the data flow (ATC)')} />}
      </div>
    </Modal>
  );
}

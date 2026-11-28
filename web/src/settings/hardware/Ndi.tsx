// Réglages › NDI : en deux onglets, comme les flux réseau. Sources · reçues : la bibliothèque, le
// réseau NDI et les sources vues (avec leur aperçu). Destinations · envoyées : les flux du serveur
// que le Core envoie en NDI (celui qu'une appli écrit, ou une entrée SDI, ST 2110, réseau).
import { useState } from 'react';
import { Network as NetIcon, Pencil, Plus, Radio, RefreshCw, Search, Send, Trash2 } from 'lucide-react';
import { Badge, Card, Dropdown, Empty, Field, FieldGrid, Modal, PageHead, SubTabs, Switch, confirmDialog, t, tn } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';
import { HwSaveBar, useHardware } from './common';
import { Preview, Tile } from './Inputs';
import { useMounted, usePoll, useRepeat } from '../../poll';
import { nicOptions } from './Hardware';
import { editHw, type NdiOutput } from './store';

const enc = encodeURIComponent;

interface OutStatus {
  state: 'starting' | 'live' | 'black' | 'error';
  format?: string | null;
  frames: number;
  connections?: number | null;
  audio: boolean;
  channels: number;
  error?: string | null;
}

interface OutputsView {
  outputs: (NdiOutput & { flow: string; status?: OutStatus | null; mxlFlow?: string })[];
  sources: { flow: string; label: string; kind: string; output?: string }[];
  on: boolean;
  mxl: boolean;
  sendError?: string | null;
}

export function NdiPage() {
  const hw = useHardware();
  const [tab, setTab] = useState<'sources' | 'destinations'>('sources');
  const [editing, setEditing] = useState<NdiOutput | null>(null);
  const view = usePoll<OutputsView>('/api/hardware/ndi/outputs', tab === 'destinations' ? 1000 : 5000);
  if (!hw) return null;
  const [snap, draft] = hw;
  const outs = draft.ndiOutputs ?? [];
  const newOutput: NdiOutput = { id: '', name: '', enabled: true, source: '', groups: '' };
  return (
    <>
      <PageHead title="NDI" sub={snap.ndi.path ? `${t('NDI library')}${snap.ndi.version ? ` · ${snap.ndi.version}` : ''}` : t('NDI library not installed')} />
      <SubTabs<'sources' | 'destinations'>
        items={[
          { id: 'sources', label: t('Sources · received') },
          { id: 'destinations', label: t('Destinations · sent'), count: outs.length },
        ]}
        value={tab}
        onChange={setTab}
      >
        {tab === 'destinations' && (
          <button type="button" className="st-btn primary" onClick={() => setEditing(newOutput)}>
            <Plus size={14} /> {t('New destination')}
          </button>
        )}
      </SubTabs>

      {tab === 'sources' && (
        <>
          <Card icon={<Radio size={16} />} title={t('NDI library')}>
            {snap.ndi.path ? (
              <div className="core-service">
                <Badge tone="ok">{t('Installed')}</Badge>
                <span className="st-mono st-muted">{snap.ndi.path}</span>
              </div>
            ) : (
              <p className="st-muted core-note">{t('The NDI library (libndi) is not installed: the installer sets it up, after its licence is accepted.')}</p>
            )}
          </Card>
          <Card icon={<NetIcon size={16} />} title={t('NDI network')}>
            <FieldGrid>
              <Field label={t('Interface')} hint={t('Also set in Network.')} group>
                <Dropdown value={draft.network.ndi} options={nicOptions(snap.nics)} onChange={(v) => editHw((x) => void (x.network.ndi = v))} />
              </Field>
              <Field label={t('Discovery server')} hint={t('Empty: the sources announce themselves (mDNS).')}>
                <input className="st-mono" value={draft.ndi.discovery} placeholder="10.0.0.2" onChange={(e) => editHw((x) => void (x.ndi.discovery = e.target.value))} />
              </Field>
              <Field label={t('Machine name')} hint={t('Receivers see “NAME (source)”.')}>
                <input value={draft.ndi.machine} placeholder={t('The computer’s')} onChange={(e) => editHw((x) => void (x.ndi.machine = e.target.value))} />
              </Field>
            </FieldGrid>
          </Card>
          {snap.ndi.path && <NdiSources />}
        </>
      )}

      {tab === 'destinations' && (
        <>
          {view && !view.on && <p className="core-warn">{t('NDI is off in Features: nothing is sent.')}</p>}
          {view?.on && view.sendError && outs.some((o) => o.enabled) && <p className="core-warn">{t('Nothing is sent in NDI: {why}', { why: friendly(view.sendError) })}</p>}
          {view && !view.mxl && outs.length > 0 && <p className="core-warn">{t('MXL is not available: the NDI destinations need it.')}</p>}
          {outs.length === 0 ? (
            <Empty icon={Send} text={t('No NDI destination')} />
          ) : (
            <div className="core-inputs">
              {outs.map((o) => {
                const live = view?.outputs.find((x) => x.id === o.id);
                // Le statut n'est juste que pour ce qui est enregistré (pas un brouillon renommé).
                const st = live && live.name === o.name ? live.status : null;
                const src = o.source ? (view?.sources.find((s) => s.flow === o.source)?.label ?? o.source) : t('An app');
                const sub = [src, st?.format, st?.audio ? tn(st.channels, '{n} audio channel', '{n} audio channels') : st ? t('no sound') : null].filter(Boolean).join(' · ');
                return (
                  <Tile
                    key={o.id || o.name}
                    pic={<Preview src={`/api/hardware/ndi/outputs/${enc(o.id)}/preview.jpg`} on={st?.state === 'live'} empty={st?.state === 'black' ? t('Black and silence') : o.enabled ? t('No picture') : t('Off')} />}
                    head={
                      <>
                        <strong className="core-input-wire">{o.name}</strong>
                        {!o.enabled ? <Badge>{t('Off')}</Badge> : stateBadge(st)}
                      </>
                    }
                    sub={sub}
                    error={o.enabled && st?.error ? friendly(st.error) : null}
                    foot={
                      <div className="core-stream-foot">
                        <span className="st-muted">
                          {st?.connections != null ? tn(st.connections, '{n} receiver connected', '{n} receivers connected') : o.groups ? t('Groups: {groups}', { groups: o.groups }) : t('Public group')}
                        </span>
                        {!o.source && live?.mxlFlow && (
                          <span className="st-muted">
                            {t('MXL flow written by the app:')} <span className="st-mono">{live.mxlFlow}</span>
                          </span>
                        )}
                        <span className="core-stream-actions">
                          <Switch checked={o.enabled} onChange={(on) => editHw((x) => void (x.ndiOutputs = (x.ndiOutputs ?? []).map((y) => (y.id === o.id ? { ...y, enabled: on } : y))))} />
                          <button type="button" className="st-icon-btn ghost" title={t('Edit')} onClick={() => setEditing(o)}>
                            <Pencil size={14} />
                          </button>
                          <button
                            type="button"
                            className="st-icon-btn ghost"
                            title={t('Delete')}
                            onClick={async () => {
                              if (await confirmDialog({ title: t('Delete {name}?', { name: o.name }), confirm: t('Delete'), danger: true })) {
                                editHw((x) => void (x.ndiOutputs = (x.ndiOutputs ?? []).filter((y) => y.id !== o.id)));
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
          )}
        </>
      )}

      {editing && (
        <OutputEditor
          value={editing}
          sources={view?.sources ?? []}
          taken={outs.filter((o) => o.id !== editing.id).map((o) => o.id)}
          onClose={() => setEditing(null)}
          onSave={(v) => {
            editHw((x) => {
              const list = [...(x.ndiOutputs ?? [])];
              const k = editing.id ? list.findIndex((y) => y.id === editing.id) : -1;
              if (k < 0) list.push(v);
              else list[k] = v;
              x.ndiOutputs = list;
            });
            setEditing(null);
          }}
        />
      )}
      <HwSaveBar />
    </>
  );
}

function stateBadge(st: OutStatus | null | undefined) {
  switch (st?.state) {
    case 'live':
      return <Badge tone="ok">{t('On air')}</Badge>;
    case 'black':
      return <Badge tone="warn">{t('Black on air')}</Badge>;
    case 'error':
      return <Badge tone="bad">{t('Error')}</Badge>;
    default:
      return <Badge>{st ? t('Starting…') : t('Not saved')}</Badge>;
  }
}

/** Un identifiant d'après le nom : « Caméra 2 » → « camera-2 ». */
function slug(name: string, taken: string[]): string {
  const base =
    name
      .normalize('NFD')
      .replace(/[̀-ͯ]/g, '')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '')
      .slice(0, 40) || 'ndi';
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
  return id;
}

function OutputEditor({ value, sources, taken, onClose, onSave }: { value: NdiOutput; sources: OutputsView['sources']; taken: string[]; onClose: () => void; onSave: (v: NdiOutput) => void }) {
  const [v, setV] = useState<NdiOutput>(value);
  const set = <K extends keyof NdiOutput>(k: K, x: NdiOutput[K]) => setV({ ...v, [k]: x });
  const name = v.name.trim();
  const valid = name && !/[()]/.test(name);
  return (
    <Modal
      title={value.name ? t('Edit {name}', { name: value.name }) : t('New NDI destination')}
      icon={<Send size={16} />}
      onClose={onClose}
      onSubmit={() => valid && onSave({ ...v, name, id: v.id || slug(name, taken) })}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={!valid}>
            {t('OK')}
          </button>
        </>
      }
    >
      <FieldGrid>
        <Field label={t('NDI name')} hint={t('Receivers see “MACHINE (name)”. No brackets.')}>
          <input value={v.name} placeholder={t('Programme, Clean feed…')} onChange={(e) => set('name', e.target.value)} />
        </Field>
        <Field label={t('What is sent')} group>
          <Dropdown
            value={v.source}
            options={[
              { value: '', label: t('An app (writes the stream in MXL)') },
              ...sources.filter((s) => !s.output || s.output !== value.id).map((s) => ({ value: s.flow, label: s.kind === 'app' || s.kind === 'ndi' ? t('Same stream as {name}', { name: s.label }) : s.label })),
            ]}
            onChange={(x) => set('source', x)}
          />
        </Field>
        <Field label={t('NDI groups')} hint={t('Separated by commas. Empty: the public group.')}>
          <input value={v.groups} placeholder="public" onChange={(e) => set('groups', e.target.value)} />
        </Field>
        <Switch checked={v.enabled} onChange={(on) => set('enabled', on)} label={t('Sent')} />
      </FieldGrid>
    </Modal>
  );
}

function NdiSources() {
  const [sources, setSources] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [looking, setLooking] = useState(false);
  const mounted = useMounted();
  // `alive` : faux une fois la page quittée (ou la recherche régulière arrêtée) ; rien ne change alors.
  const look = async (alive: () => boolean = mounted) => {
    setLooking(true);
    try {
      const r = await api<{ sources: string[] }>('GET', '/api/hardware/ndi/sources');
      if (alive()) {
        setSources((cur) => (JSON.stringify(cur) === JSON.stringify(r.sources) ? cur : r.sources));
        setError(null);
      }
    } catch (e) {
      if (alive()) setError(friendly(e));
    }
    if (mounted()) setLooking(false);
  };
  useRepeat('ndi', 10000, (alive) => look(alive));
  return (
    <Card
      icon={<Search size={16} />}
      title={sources ? tn(sources.length, '{n} visible source', '{n} visible sources') : t('Visible sources')}
      actions={
        <button type="button" className="st-btn small" disabled={looking} onClick={() => void look()}>
          <RefreshCw size={13} className={looking ? 'st-spin' : undefined} /> {t('Look again')}
        </button>
      }
    >
      {error && <p className="core-warn">{error}</p>}
      {sources === null ? (
        <span className="st-muted">{t('Looking…')}</span>
      ) : sources.length === 0 ? (
        <span className="st-muted">{t('No source seen')}</span>
      ) : (
        <div className="core-inputs">
          {sources.map((name) => {
            const m = /^(.*?) \((.*)\)$/.exec(name);
            return (
              <Tile
                key={name}
                pic={<Preview src={`/api/hardware/ndi/${enc(name)}/preview.jpg`} on empty={t('No picture')} />}
                head={<strong>{m ? m[2] : name}</strong>}
                sub={m ? m[1] : undefined}
                foot={null}
              />
            );
          })}
        </div>
      )}
    </Card>
  );
}

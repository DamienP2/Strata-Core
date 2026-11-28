// Réglages › Ce serveur › Fonctions : les fonctions du Core qu'on active ou qu'on coupe (ST 2110,
// JPEG XS, IPMX et ses options, Dante, NDI, flux réseau). Une fonction coupée disparaît des
// réglages et ne tourne plus ; une bibliothèque ou une licence qui manque est dite, sans bloquer.
import { useRef } from 'react';
import { create } from 'zustand';
import { FileKey, Trash2, Upload } from 'lucide-react';
import { Badge, Card, PageHead, Switch, confirmDialog, t, toast } from '@strata/ui';
import { ApiError, api } from '../api';
import { friendly } from '../errors';

export interface Feature {
  id: string;
  name: string;
  /** Réglée active (sans regarder celles qu'elle demande). */
  chosen: boolean;
  /** Active, et celles qu'elle demande aussi. */
  on: boolean;
  default: boolean;
  needs: string[];
  available: boolean;
  why?: string | null;
  license?: { present: boolean; bytes?: number | null; sha256?: string | null } | null;
}

/** Les fonctions, relues par la page et par la navigation (une coupée n'y est plus). */
export const useFeatures = create<{ list: Feature[] | null }>(() => ({ list: null }));

export async function loadFeatures() {
  try {
    const r = await api<{ features: Feature[] }>('GET', '/api/features');
    useFeatures.setState({ list: r.features });
  } catch {
    /* pas administrateur, ou serveur absent : tout reste montré */
  }
}

/** Active (vrai tant que la liste n'est pas lue : rien ne disparaît par erreur). */
export function featureOn(list: Feature[] | null, id: string) {
  return list?.find((f) => f.id === id)?.on ?? true;
}

const ABOUT: Record<string, string> = {
  st2110: 'Uncompressed video, audio and data over IP (SMPTE ST 2110), with NMOS.',
  jpegxs: 'Compressed video flows (ST 2110-22), to choose per flow.',
  ipmx: 'Pro AV over IP (VSF TR-10): IPMX signalling for the ST 2110 flows.',
  'ipmx-privacy': 'Encrypted IPMX flows (TR-10-13).',
  hdcp: 'Protected content on IPMX (TR-10-5). Needs the keys of a DCP LLC license.',
  dante: 'Audio from and to Dante devices in AES67 mode.',
  ndi: 'NDI sources and outputs for the apps.',
  streams: 'SRT, RTMP, HLS, RTSP, UDP inputs and destinations, and the HTML5 player.',
};

const GROUPS = [
  { title: 'Media over IP', ids: ['st2110', 'jpegxs', 'ipmx', 'ipmx-privacy', 'hdcp', 'dante'] },
  { title: 'Other sources and outputs', ids: ['ndi', 'streams'] },
];

export function FeaturesPage() {
  const list = useFeatures((s) => s.list);
  if (!list) {
    void loadFeatures();
    return null;
  }
  const set = async (f: Feature, on: boolean) => {
    try {
      const r = await api<{ features: Feature[] }>('PUT', `/api/features/${encodeURIComponent(f.id)}`, { on });
      useFeatures.setState({ list: r.features });
      if (f.id === 'st2110' && on && !f.on) toast(t('ST 2110 comes back after a restart of the Core'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const nameOf = (id: string) => t(list.find((f) => f.id === id)?.name ?? id);
  return (
    <>
      <PageHead title={t('Features')} />
      {GROUPS.map((g) => (
        <Card key={g.title} title={t(g.title)}>
          <div className="core-features">
            {list
              .filter((f) => g.ids.includes(f.id))
              .map((f) => (
                <div key={f.id} className={`core-feature${f.needs.length ? ' sub' : ''}`}>
                  <div className="core-feature-head">
                    <Switch checked={f.chosen} onChange={(on) => void set(f, on)} label={<strong>{t(f.name)}</strong>} />
                    {f.on ? (
                      f.available ? (
                        <Badge tone="ok">{t('Active')}</Badge>
                      ) : (
                        <Badge tone="warn">{t('Not available')}</Badge>
                      )
                    ) : f.chosen ? (
                      <Badge>{t('Waiting for {name}', { name: f.needs.filter((n) => !featureOn(list, n)).map(nameOf).join(', ') })}</Badge>
                    ) : (
                      <Badge>{t('Off')}</Badge>
                    )}
                  </div>
                  <p className="st-muted">{t(ABOUT[f.id] ?? '')}</p>
                  {f.chosen && !f.available && f.why && <p className="core-warn">{friendly(f.why)}</p>}
                  {f.license && <License f={f} onList={(l) => useFeatures.setState({ list: l })} />}
                </div>
              ))}
          </div>
        </Card>
      ))}
    </>
  );
}

/** La licence d'une fonction (HDCP : le fichier de clés fourni par DCP LLC). */
function License({ f, onList }: { f: Feature; onList: (l: Feature[]) => void }) {
  const picker = useRef<HTMLInputElement>(null);
  const send = async (file: File) => {
    try {
      const res = await fetch(`/api/features/${encodeURIComponent(f.id)}/license`, { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/octet-stream' }, body: file });
      if (!res.ok) throw new ApiError(res.status, await res.text());
      onList(((await res.json()) as { features: Feature[] }).features);
      toast(t('License added'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const remove = async () => {
    if (!(await confirmDialog({ title: t('Remove the license?'), confirm: t('Remove'), danger: true }))) return;
    try {
      onList((await api<{ features: Feature[] }>('DELETE', `/api/features/${encodeURIComponent(f.id)}/license`)).features);
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const l = f.license!;
  return (
    <div className="core-feature-license">
      <FileKey size={14} />
      {l.present ? (
        <span>
          {t('License')} <span className="st-mono">{l.sha256}</span> · {l.bytes} {t('bytes')}
        </span>
      ) : (
        <span className="st-muted">{t('No license')}</span>
      )}
      <span className="core-feature-tools">
        <button type="button" className="st-btn small" onClick={() => picker.current?.click()}>
          <Upload size={13} /> {l.present ? t('Replace') : t('Add the license')}
        </button>
        {l.present && (
          <button type="button" className="st-icon-btn ghost" title={t('Remove')} onClick={() => void remove()}>
            <Trash2 size={14} />
          </button>
        )}
      </span>
      <input ref={picker} type="file" hidden onChange={(e) => e.target.files?.[0] && void send(e.target.files[0])} />
    </div>
  );
}

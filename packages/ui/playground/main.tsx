// Banc d'essai : chaque composant du paquet, dans les deux thèmes et les deux langues.
import { StrictMode, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Cloud, Inbox, Plus, Server, SlidersHorizontal, Trash2 } from 'lucide-react';
import {
  Badge, Brand, Card, Choice, DialogHost, Dot, Empty, Field, FieldGrid, InfoTip, LANGS, SaveBar, Segmented, Switch, Toasts,
  alertDialog, confirmDialog, installEscape, installTooltips, promptDialog, setLang, setTheme, t, tc, toast, useLang, useTheme,
  type ThemePref,
} from '../src';
import '../src/styles/fonts.css';
import './playground.css';

installTooltips();
installEscape();

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="pg-section">
      <h3>{title}</h3>
      <div className="pg-row">{children}</div>
    </section>
  );
}

function App() {
  useLang((s) => s.lang);
  const theme = useTheme((s) => s.theme);
  const lang = useLang((s) => s.lang);
  const [on, setOn] = useState(true);
  const [mode, setMode] = useState<'master' | 'member'>('master');
  const [name, setName] = useState('Régie A');
  const dirty = name !== 'Régie A';

  return (
    <div className="pg">
      <nav className="pg-top">
        <Brand module="Core" />
        <div className="st-spacer" />
        <Choice<ThemePref> value={theme} onChange={setTheme} options={(['dark', 'light', 'system'] as const).map((v) => ({ value: v, label: tc('theme', v[0].toUpperCase() + v.slice(1)) }))} />
        <Choice value={lang} onChange={setLang} options={LANGS.map((l) => ({ value: l.id, label: l.label }))} />
      </nav>
      <main className="st-page pg-main">
        <Section title="Marque">
          <Brand module="LiveCut" />
          <Brand module="Graphics" />
          <Brand module="Core" stacked />
        </Section>

        <Section title="Boutons">
          <button className="st-btn primary"><Plus size={14} /> Nouveau</button>
          <button className="st-btn">Annuler</button>
          <button className="st-btn ghost">Fantôme</button>
          <button className="st-btn on">Actif</button>
          <button className="st-btn danger"><Trash2 size={14} /> Supprimer</button>
          <button className="st-btn danger-solid">Supprimer définitivement</button>
          <button className="st-btn warn">Attention</button>
          <button className="st-btn small">Petit</button>
          <button className="st-btn" disabled>Désactivé</button>
          <button className="st-icon-btn" title="Réglages"><SlidersHorizontal size={14} /></button>
          <button className="st-icon-btn ghost" title="Supprimer"><Trash2 size={14} /></button>
        </Section>

        <Section title="États">
          <Badge>Inactive</Badge>
          <Badge tone="ok">En marche</Badge>
          <Badge tone="warn">Mise à jour</Badge>
          <Badge tone="bad">Injoignable</Badge>
          <Badge tone="accent">Maître</Badge>
          <span className="pg-dots"><Dot tone="ok" title="Joignable" /> <Dot tone="warn" title="1 alerte" /> <Dot tone="bad" title="Injoignable" /> <Dot title="Inactif" /></span>
          <span>Infobulle dessinée <InfoTip text="Le title de l’élément, affiché dans une carte qui suit le pointeur." /></span>
        </Section>

        <Section title="Contrôles">
          <Switch checked={on} onChange={setOn} label="Active" />
          <Segmented value={mode} onChange={setMode} options={[{ value: 'master', label: 'Maître' }, { value: 'member', label: 'Membre' }]} />
        </Section>

        <Section title="Réglages">
          <div className="pg-wide">
            <Card icon={<Server size={16} />} title="Serveur" actions={<button className="st-btn small"><Cloud size={13} /> Vérifier</button>}>
              <FieldGrid>
                <Field label="Nom" hint="Vu par les utilisateurs dans le cockpit">
                  <input value={name} onChange={(e) => setName(e.target.value)} />
                </Field>
                <Field label="Nom DNS">
                  <input value="strata01.regie.exemple.fr" readOnly />
                </Field>
                <Field label="Rôle" group>
                  <Segmented value={mode} onChange={setMode} options={[{ value: 'master', label: 'Maître' }, { value: 'member', label: 'Membre' }]} />
                </Field>
              </FieldGrid>
            </Card>
          </div>
        </Section>

        <Section title="État vide">
          <div className="pg-empty">
            <Empty icon={Inbox} text="Aucun serveur rattaché">
              <button className="st-btn small"><Plus size={13} /> Ajouter un serveur</button>
            </Empty>
          </div>
        </Section>

        <Section title="Dialogues et messages">
          <button className="st-btn" onClick={async () => toast((await confirmDialog({ title: 'Détacher Régie B ?', message: 'Son cockpit se rallume.', confirm: 'Détacher', danger: true })) ? 'Détaché' : 'Annulé')}>Confirmer</button>
          <button className="st-btn" onClick={async () => { const v = await promptDialog({ title: 'Renommer le serveur', label: 'Nom', value: 'Régie B' }); if (v) toast(`Renommé en ${v}`); }}>Demander</button>
          <button className="st-btn" onClick={() => alertDialog({ title: 'Code d’invitation', message: 'K7Q-4XM-92P\nValable 10 min' })}>Informer</button>
          <button className="st-btn" onClick={() => toast('Mise à jour installée')}>Toast</button>
          <button className="st-btn" onClick={() => toast('Studio injoignable', 'error')}>Toast d’erreur</button>
          <span className="st-muted">{t('Close')} · {t('Cancel')}</span>
        </Section>
      </main>
      <SaveBar dirty={dirty} saving={false} onSave={() => toast('Enregistré')} onDiscard={() => setName('Régie A')} />
      <DialogHost />
      <Toasts />
    </div>
  );
}

setLang('fr');
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

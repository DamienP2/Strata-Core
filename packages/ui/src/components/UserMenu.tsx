/*
 * Le menu de l'utilisateur : avatar et nom ; qui, langue, thème, raccourcis clavier, déconnexion. Langue
 * et thème s'appliquent tout de suite ; onLang / onTheme les font garder sur le serveur pour l'utilisateur.
 * Repris de LiveCut ; l'utilisateur et les actions arrivent par les props.
 */
import { useEffect, useRef, useState } from 'react';
import { ChevronDown, Keyboard, Languages, LogOut, SunMoon } from 'lucide-react';
import { LANGS, setLang, t, tc, useLang, type Lang } from '../i18n';
import { setTheme, THEMES, useTheme, type ThemePref } from '../theme';
import { useMenuKeys } from '../menuKeys';

const THEME_LABEL: Record<ThemePref, string> = { dark: 'Dark', light: 'Light', system: 'System' };

/** La lettre de l'avatar. */
export const initial = (name: string) => (name.trim()[0] ?? '?').toUpperCase();

export function UserMenu({ name, role, named = false, onLang, onTheme, onShortcuts, onSignOut }: {
  name: string;
  /** La ligne sous le nom (« Administrateur des serveurs », ses groupes…). */
  role?: string;
  /** Le nom à côté de l'avatar (sinon l'avatar seul). */
  named?: boolean;
  onLang?: (l: Lang) => void;
  onTheme?: (t: ThemePref) => void;
  onShortcuts?: () => void;
  onSignOut: () => void;
}) {
  const lang = useLang((s) => s.lang);
  const theme = useTheme((s) => s.theme);
  const [open, setOpen] = useState(false);
  const btn = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const close = useRef((focus: boolean) => {
    setOpen(false);
    if (focus) btn.current?.focus();
  }).current;
  useMenuKeys(open, menu, close);
  useEffect(() => {
    if (open) menu.current?.querySelector<HTMLElement>('.st-choice button.on')?.focus();
  }, [open]);

  return (
    <div className="st-user">
      <button
        ref={btn}
        type="button"
        className={`st-user-btn ${named ? 'named' : ''} ${open ? 'on' : ''}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title={named ? undefined : name}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (!open && e.key === 'ArrowDown') {
            e.preventDefault();
            setOpen(true);
          }
        }}
      >
        <span className="st-avatar">{initial(name)}</span>
        {named && <span className="st-user-name">{name}</span>}
        <ChevronDown size={13} />
      </button>
      {open && (
        <div ref={menu} className="st-menu st-user-menu" role="menu">
          <div className="st-user-who">
            <span className="st-avatar">{initial(name)}</span>
            <div>
              <b>{name}</b>
              {role && <small>{role}</small>}
            </div>
          </div>
          <div className="st-menu-sep" />
          <div className="st-menu-row">
            <Languages size={14} /> {t('Language')}
            <div className="st-choice" role="group" aria-label={t('Language')}>
              {LANGS.map((l) => (
                <button key={l.id} type="button" className={lang === l.id ? 'on' : ''} aria-pressed={lang === l.id} title={l.name} onClick={() => (setLang(l.id), onLang?.(l.id))}>
                  {l.label}
                </button>
              ))}
            </div>
          </div>
          <div className="st-menu-row">
            <SunMoon size={14} /> {t('Theme')}
            <div className="st-choice" role="group" aria-label={t('Theme')}>
              {THEMES.map((th) => (
                <button key={th} type="button" className={theme === th ? 'on' : ''} aria-pressed={theme === th} onClick={() => (setTheme(th), onTheme?.(th))}>
                  {tc('theme', THEME_LABEL[th])}
                </button>
              ))}
            </div>
          </div>
          {onShortcuts && (
            <button
              type="button"
              role="menuitem"
              className="st-menu-item"
              onClick={() => {
                setOpen(false);
                onShortcuts();
              }}
            >
              <Keyboard size={14} /> {t('Keyboard shortcuts')}
              <span className="st-keycap">?</span>
            </button>
          )}
          <div className="st-menu-sep" />
          <button
            type="button"
            role="menuitem"
            className="st-menu-item danger"
            onClick={() => {
              setOpen(false);
              onSignOut();
            }}
          >
            <LogOut size={14} /> {t('Sign out')}
          </button>
        </div>
      )}
    </div>
  );
}

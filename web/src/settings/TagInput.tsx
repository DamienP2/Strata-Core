// Une liste de mots (groupes de l'annuaire, domaines) : des étiquettes, une case pour en ajouter
// (Entrée ou virgule), × pour en retirer.
import { useState } from 'react';
import { X } from 'lucide-react';
import { t } from '@strata/ui';

export function TagInput({ value, onChange, placeholder }: { value: string[]; onChange: (v: string[]) => void; placeholder?: string }) {
  const [text, setText] = useState('');
  const add = () => {
    const words = text.split(',').map((w) => w.trim()).filter(Boolean);
    if (!words.length) return;
    onChange([...value, ...words.filter((w) => !value.some((v) => v.toLowerCase() === w.toLowerCase()))]);
    setText('');
  };
  return (
    <div className="core-tags">
      {value.map((v) => (
        <span key={v} className="core-tag">
          {v}
          <button type="button" title={t('Remove')} onClick={() => onChange(value.filter((x) => x !== v))}>
            <X size={11} />
          </button>
        </span>
      ))}
      <input
        value={text}
        placeholder={value.length ? '' : placeholder}
        onChange={(e) => setText(e.target.value)}
        onBlur={add}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ',') {
            e.preventDefault();
            add();
          } else if (e.key === 'Backspace' && !text && value.length) onChange(value.slice(0, -1));
        }}
      />
    </div>
  );
}

// @strata/ui : le design commun des applis Strata.
// Le CSS s'importe à part : import '@strata/ui/styles.css';
export * from './theme';
export * from './i18n';
export { Brand, FigaroF, AudioVisuel } from './components/Brand';
export { Empty, InfoTip, Switch, Segmented, Choice, Badge, Dot, type SegOption, type Tone } from './components/basics';
export { Card, Field, FieldGrid, SaveBar, useLeaveGuard, canLeave } from './components/settings';
export { confirmDialog, promptDialog, alertDialog, DialogHost, type ConfirmOptions, type PromptOptions, type AlertOptions } from './components/dialogs';
export { toast, Toasts, type ToastKind } from './components/Toasts';
export { installTooltips } from './tooltips';
export { installEscape } from './escape';
import './styles/index.css';

// Le thème gardé, avant le premier affichage (pas d'éclair de l'autre thème). Un fichier à part :
// la politique de sécurité du serveur (CSP) refuse les scripts écrits dans la page.
try {
  var t = localStorage.getItem('strata.theme');
  if (t === 'light' || (t === 'system' && matchMedia('(prefers-color-scheme: light)').matches)) document.documentElement.dataset.theme = 'light';
} catch (e) {}

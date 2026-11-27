# Installer un serveur Strata

Sur Ubuntu 24.04 (x86-64), en SSH :

```bash
curl -fsSL https://github.com/DamienP2/Strata-Core/releases/latest/download/install.sh | sudo bash
```

`install.sh` télécharge l'installateur (`strata-install`) de la dernière release, vérifie son
empreinte et le lance. Un écran plein terminal, au clavier :

1. **Système** : Ubuntu 24.04, processeur, mémoire, disque, internet, systemd, droits.
2. **Serveur** : nom d'usage (« Régie A »), nom DNS, fuseau. S'il n'est pas résolu, l'installateur
   donne les deux enregistrements à créer (le nom et `*.le nom`).
3. **Applis** : dépôts GitHub (`owner/repo`) ou archives, installées et activées.
4. **Matériel** :
   - **DeckLink** : le pilote Desktop Video de Blackmagic n'est pas redistribuable. L'installateur
     donne la marche à suivre (le télécharger sur son ordinateur, le copier par `scp` avec la
     commande exacte) et attend l'archive ; il installe le paquet avec DKMS.
   - **NDI** : le SDK NDI est téléchargé, sa licence affichée ; la bibliothèque n'est installée
     qu'une fois la licence acceptée.
5. **Réglages** : ouvrir les ports 80 et 443 si ufw est actif.
6. **Compte administrateur**.
7. **Récapitulatif**, puis l'installation, une ligne par étape.

Journal complet : `/var/log/strata-install.log`.

## Ce qui est posé

| | |
| --- | --- |
| `/opt/strata/bin/strata-core` | le Core (au compte du service : il se met à jour lui-même) ; `strata-core` dans le PATH |
| `/var/lib/strata-core` | ses données |
| `strata-core.service` | le service, sous le compte `strata`, port 80 (et 443, 161) |
| `strata-ptp4l.service` | ptp4l, démarré seulement si le PTP est réglé (Matériel › Horloge) |
| `/etc/strata/`, `/etc/chrony/conf.d/strata.conf` | ce que le Core écrit pour l'heure |
| `/etc/polkit-1/rules.d/50-strata-core.rules` | le compte `strata` peut relancer chrony et ptp4l, rien d'autre |
| `/etc/sysctl.d/60-strata.conf` | des tampons réseau larges (flux UDP) |

## Relancé sur un serveur installé

Il propose : mettre à jour le Core, ajouter des applis, installer DeckLink / NDI, réparer (fichiers
du système et droits), désinstaller (données gardées).

## Sans écran

Les mêmes choix en options (`strata-install --help`) :

```bash
curl -fsSL https://github.com/DamienP2/Strata-Core/releases/latest/download/install.sh \
  | sudo STRATA_ADMIN_PASSWORD='…' bash -s -- --yes --dns-name strata01.regie.exemple.fr \
      --name "Régie A" --admin admin --apps DamienP2/strata-cut --desktop-video auto
```

- `--package <archive>` : le Core depuis son archive (serveur sans internet) ;
- `--ndi --accept-ndi-license` : la bibliothèque NDI ;
- `--update`, `--repair`, `--uninstall [--purge]`.

## Essai à blanc

`--root <dossier>` : tout est posé sous ce dossier, les commandes du système sont seulement écrites
au journal ; le Core, lui, est vraiment lancé (réglages, compte, applis).

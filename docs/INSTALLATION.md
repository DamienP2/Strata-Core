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
     commande exacte) et l'attend : l'archive telle que téléchargée (`.tar.gz`), son dossier
     déballé, ou ce qu'elle contient (`.deb`, installateur `.sh`). Il installe le pilote avec DKMS.
   - **NDI** : à mettre sur *Oui*. Le SDK NDI est téléchargé (ou pris sur le serveur s'il y a été
     copié : `Install_NDI_SDK_v6_Linux.tar.gz`, pour un serveur sans internet), puis sa licence
     s'affiche : **A** pour l'accepter, **R** pour la refuser. La bibliothèque n'est installée
     qu'une fois la licence acceptée.
   - **GPU** : le pilote NVIDIA recommandé par Ubuntu (`ubuntu-drivers`, avec NVENC/NVDEC) ;
     VA-API pour Intel et AMD (`intel-media-va-driver-non-free` ou `mesa-va-drivers`, `vainfo`).
   - **Secure Boot** actif : les modules Blackmagic et NVIDIA doivent être signés ; au redémarrage,
     enrôlez la clé MOK (écran bleu « Perform MOK management »).
5. **Réglages** : ouvrir les ports 80 et 443 si ufw est actif.
6. **Compte administrateur**.
7. **Récapitulatif**, puis l'installation, une ligne par étape.

Journal complet : `/var/log/strata-install.log`.

## Ce qui est posé

| | |
| --- | --- |
| `/opt/strata/bin/strata-core` | le Core (au compte du service : il se met à jour lui-même) ; `strata-core` dans le PATH |
| `/var/lib/strata-core` | ses données |
| `strata-core.service` | le service, sous le compte `strata` (groupes `video` et `render` : cartes et GPU), port 80 (et 443, 161) |
| `strata-ptp4l.service` | ptp4l, démarré seulement si le PTP est réglé (Matériel › Horloge) |
| `/etc/strata/`, `/etc/chrony/conf.d/strata.conf` | ce que le Core écrit pour l'heure |
| `/opt/strata/lib` | MXL (`libmxl.so`), et sous `mtl/` Intel Media Transport Library avec DPDK (ST 2110) |
| `strata-media.service` | en root, au démarrage et quand le Core le demande : réserve les hugepages et passe les ports média en DPDK (`/etc/strata/media.json`), seulement si le Core tient le ST 2110 |
| `/etc/polkit-1/rules.d/50-strata-core.rules` | le compte `strata` peut relancer chrony, ptp4l et strata-media, rien d'autre |
| `/etc/sysctl.d/60-strata.conf` | des tampons réseau larges (flux UDP) |

Le service du Core a aussi les droits du ST 2110 par MTL : sockets brutes et AF_XDP
(`CAP_NET_RAW`, `CAP_NET_ADMIN`, `CAP_BPF`), mémoire verrouillée (`CAP_IPC_LOCK`,
`LimitMEMLOCK=infinity`), priorités (`CAP_SYS_NICE`). Un serveur mis à jour depuis le cockpit les
reçoit en relançant l'installateur (*Mettre à jour* ou *Réparer*).

## ST 2110 avec une Intel E810

*Réglages › Matériel › Réseau*, « Flux reçus et envoyés par le Core » :

- **Noyau** : rien à préparer. Pour quelques flux HD.
- **AF_XDP** : rien à préparer non plus, bien plus rapide ; le port reste au noyau (son adresse
  aussi).
- **DPDK** : le plus rapide (UHD, nombreux flux). Les ports média quittent le noyau : donnez leur
  adresse dans la page. Il faut l'IOMMU : ajoutez `intel_iommu=on iommu=pt` à
  `GRUB_CMDLINE_LINUX` dans `/etc/default/grub`, puis `sudo update-grub` et redémarrez. Le port qui
  porte la route par défaut (l'administration) n'est jamais passé en DPDK.

Le Core réserve lui-même les hugepages (2 Go, plus 512 Mo par flux HD et 2 Go par flux UHD, au
plus la moitié de la mémoire) et les rend quand on repasse à « Non ». Si la mémoire est trop
morcelée pour les réserver, redémarrez le serveur.

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
- `--gpu-drivers` : le pilote NVIDIA, ou VA-API, selon les GPU présents ;
- `--update`, `--repair`, `--uninstall [--purge]`.

## Repartir de zéro

Relancer la commande d'installation sur un serveur installé ouvre un menu : *Désinstaller*
(données gardées) ou *Tout effacer* (données, comptes, applis et le compte `strata`). Les pilotes
(Blackmagic, NDI, NVIDIA) restent. Sans écran :

```bash
curl -fsSL https://github.com/DamienP2/Strata-Core/releases/latest/download/install.sh | sudo bash -s -- --uninstall --purge
```

## Essai à blanc

`--root <dossier>` : tout est posé sous ce dossier, les commandes du système sont seulement écrites
au journal ; le Core, lui, est vraiment lancé (réglages, compte, applis).

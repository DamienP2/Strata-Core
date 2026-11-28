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

## Dans une machine virtuelle (Proxmox, VMware…)

Le Core tourne dans une machine virtuelle, à ces conditions :

| | Requis | Pourquoi |
| --- | --- | --- |
| **Processeur** | Type **`host`** (Proxmox : *Hardware › Processors › Type*, ou `qm set <id> --cpu host`), à défaut `x86-64-v3` ; puis arrêt complet de la VM et démarrage (pas un simple redémarrage depuis l'intérieur) | Le type par défaut (« kvm64 », « QEMU Virtual CPU ») cache AVX2 et AVX-512 : MXL, MTL et les conversions d'images en ont besoin pour aller vite. `grep -o -w 'avx2\|avx512f' /proc/cpuinfo` doit les montrer. `host` empêche la migration à chaud vers un hôte d'un autre processeur ; `x86-64-v3` la garde |
| **Cœurs** | 8 vCPU au moins (4 au strict minimum) ; pour du ST 2110, des cœurs réservés (Proxmox : *CPU affinity*) | MTL garde des cœurs occupés en continu pour cadencer les paquets |
| **Mémoire** | 16 Go au moins pour du ST 2110 (8 Go sinon), **sans ballooning** (Proxmox : *Ballooning Device* décoché) | Les hugepages réservées par le Core ne doivent pas être reprises par l'hôte |
| **Disque** | 40 Go, VirtIO SCSI | Le Core, les applis, les journaux, les sauvegardes |
| **Réseau de gestion** | Une carte VirtIO, ponts de l'hôte | Le cockpit, SSH, les mises à jour |
| **ST 2110 (Intel E810)** | La carte passée à la VM en **PCI passthrough** (Proxmox : *Hardware › Add › PCI Device*, « All Functions », « PCI-Express »), IOMMU actif sur l'hôte (`intel_iommu=on iommu=pt`) **et** dans la VM pour DPDK ; machine `q35` | Une carte réseau virtuelle (VirtIO) ne tient pas le débit ni le cadencement ST 2110 ; DPDK et AF_XDP veulent la vraie carte |
| **PTP** | Le port PTP passé en passthrough comme la E810 (horloge matérielle) | Une carte virtuelle n'a pas d'horloge PTP matérielle : l'heure serait trop imprécise pour le ST 2110 |
| **Cartes DeckLink** | Passées en **PCI passthrough** (PCI-Express, « All Functions ») ; le pilote Desktop Video s'installe dans la VM | Le pilote Blackmagic parle directement à la carte |
| **GPU** | En passthrough s'il encode ou décode (NVENC, VA-API) | Un GPU virtuel n'a pas les encodeurs matériels |
| **Secure Boot** | Désactivé dans l'UEFI de la VM, ou les modules (Blackmagic, NVIDIA) signés avec MOK | Sinon les pilotes ne se chargent pas |
| **Horloge** | L'agent QEMU installé ; la VM ne règle pas son heure sur l'hôte quand le PTP est utilisé | C'est le Core (chrony, ptp4l) qui tient l'heure |

L'installateur le vérifie (ligne *Instructions* : il prévient s'il manque AVX2 dans une VM). Si
une bibliothèque plante malgré tout (instruction absente), le Core démarre sans elle et un bandeau
dit quelles fonctions manquent.

## Ce qui est posé

| | |
| --- | --- |
| `/opt/strata/bin/strata-core` | le Core (au compte du service : il se met à jour lui-même) ; `strata-core` dans le PATH |
| `/var/lib/strata-core` | ses données |
| `strata-core.service` | le service, sous le compte `strata` (groupes `video` et `render` : cartes et GPU), port 80 (et 443, 161) |
| `strata-ptp4l.service` | ptp4l, démarré seulement si le PTP est réglé (Matériel › Horloge) |
| `/etc/strata/`, `/etc/chrony/conf.d/strata.conf` | ce que le Core écrit pour l'heure |
| `ffmpeg` (paquet Ubuntu) | les flux réseau (SRT, RTMP, HLS, RTSP…) reçus et envoyés par le Core |
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

La carte **Charge des ports média** (même page) suit chaque port en direct : débit reçu et envoyé
mesuré (compteurs du noyau, ou de MTL en DPDK), ce que demandent les flux réglés, la vitesse du
lien, et combien de flux de chaque format y tiennent encore (marge de 10 % ; en 2022-7 le port le
plus chargé décide). Une alerte part si un port dépasse 90 %, si son lien tombe, ou si les flux
réglés demandent plus que le lien.

## NMOS (Bitfocus Buttons, sans registre)

*Réglages › Matériel › Réseau*, carte **NMOS** : le Core s'annonce en mDNS
(`_nmos-node._tcp`, IS-04 en pair-à-pair) sur l'interface de gestion (ou celle choisie), et
répond sur le port 3212 (IS-04 Node API v1.2/v1.3, IS-05 Connection API v1.1). Le contrôleur doit
être sur le même réseau (mDNS ne traverse pas les routeurs).

- Chaque flux ST 2110 **envoyé** est un émetteur NMOS avec son SDP (`transportfile`). Ses
  adresses restent celles réglées dans *ST 2110* : le contrôleur peut le couper ou le rallumer,
  pas les changer.
- Chaque flux **reçu** est un récepteur : le contrôleur le branche sur un émetteur (SDP ou
  adresses, tout de suite ou à une heure donnée). Le branchement est écrit dans les réglages
  (adresses, format du SDP) et seul ce flux est relancé. Un badge « NMOS » le montre dans
  *ST 2110*.
- L'API n'a pas de mot de passe (comme NMOS le prévoit) : elle ne doit être joignable que depuis
  le réseau de la régie. L'installateur ouvre 3212/tcp et 5353/udp si ufw est actif.

## Dante et AES67 (SAP)

Un appareil Dante en **mode AES67** (Dante Controller : *Device Config › AES67 Config*) échange du
son avec le Core en ST 2110-30 (L24, 48 kHz, paquets de 1 ms, multicast, 8 canaux au plus par
flux). SAP sert à se trouver : *Matériel › ST 2110*, carte **Flux de son annoncés (AES67 / Dante)**.

- **Recevoir** : créer dans Dante Controller un flux multicast AES67 ; il apparaît dans la carte ;
  « Recevoir » crée le flux d'entrée tout réglé ; enregistrer. Les applis le lisent en MXL.
- **Émettre** : un flux de son envoyé (L24 · 1 ms, 8 canaux au plus) et **Annoncer nos flux de son
  (SAP)** : il apparaît dans Dante Controller comme émetteur ; le router vers l'appareil Dante.
- **Horloge** : le même grand maître PTP des deux côtés (Dante en AES67 : PTP par défaut, domaine
  0 ; le 2110 souvent domaine 127). La carte montre l'horloge annoncée par chaque appareil.
- Le Dante natif (hors AES67) demande une licence Audinate : il n'est pas pris en charge.

## Sauvegardes

*Ce serveur › Sauvegardes* : les réglages du Core (comptes, groupes, SSO, nom et HTTPS, cartes
SDI, réseau, flux ST 2110 et réseau, horloge, NDI, NMOS, alertes, SNMP) dans une archive
`.tar.gz`, gardée dans `/var/lib/strata-core/backups`. Une par jour (les 14 dernières), une à la
demande (avec les applis et leurs données si on le veut), et une juste avant chaque restauration.

- **Télécharger** une sauvegarde, l'**envoyer** sur un autre serveur, la **restaurer** : le Core
  redémarre sur les réglages de la sauvegarde, et chacun se reconnecte.
- L'identité du serveur (sa clé, son rattachement au cockpit) n'est restaurée que si on le
  demande : seulement pour le même serveur, réinstallé.
- Les archives contiennent des secrets (empreintes des mots de passe, clés, mots de passe du SSO
  et de l'e-mail) : à garder en lieu sûr.

## Recette

La check-list des essais sur le vrai matériel : [RECETTE.md](RECETTE.md). Les essais sans appli
(mire et bip sur une sortie, niveau du son d'une entrée) : `sudo -u strata strata-core test --help`.

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

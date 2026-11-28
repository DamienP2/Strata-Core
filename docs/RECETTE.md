# Recette d'un serveur Strata

À dérouler sur le vrai serveur (Ubuntu 24.04, DeckLink, Intel E810, GPU). Chaque point dit quoi
faire et ce qu'on doit voir. Ce qui ne va pas : noter le numéro, ce qu'on a vu, et joindre
l'export des journaux (*Réglages › Santé › Journaux › Exporter*) ou
`journalctl -u strata-core --since "-30 min"`.

Les essais sans appli passent par `strata-core test`, sous le compte du service (les flux MXL
sont à lui) :

```bash
sudo -u strata strata-core test bars sdi/<carte>/<connecteur>     # mire + bip sur une sortie SDI
sudo -u strata strata-core test bars 2110/<flux> --format 1080i50 # mire sur un flux 2110 envoyé
sudo -u strata strata-core test tone 2110/<flux> --channels 8     # bip sur un flux de son envoyé
sudo -u strata strata-core test level sdi/<carte>/<connecteur>    # niveau du son d'une entrée
sudo -u strata strata-core test level 2110/<flux>                 # niveau d'un flux de son reçu
```

`<carte>` est l'identifiant de la carte (*Matériel › Cartes SDI*, ou `cards[].id` de
`/api/hardware/shared`), `<flux>` celui du flux 2110 (`flow-1`…). Le bip est un 1 kHz à
-18 dBFS crête : un afficheur RMS lit -21. La mire fait un éclair blanc chaque seconde, et le bip
se tait pendant cette image : sur un afficheur, l'éclair et le trou du son doivent tomber
ensemble.

## 1. Installation

| | Faire | Attendu |
| --- | --- | --- |
| 1.1 | `curl -fsSL https://github.com/DamienP2/Strata-Core/releases/latest/download/install.sh \| sudo bash` | Toutes les étapes vertes ; `/var/log/strata-install.log` sans erreur |
| 1.2 | Desktop Video (DKMS), NDI, pilote GPU pendant l'installation ; redémarrer (MOK si Secure Boot) | `lsmod \| grep blackmagic`, `nvidia-smi` ou `vainfo` répondent |
| 1.3 | Ouvrir `http://<nom DNS>` | Écran de connexion, puis le cockpit |
| 1.4 | *Ce serveur › HTTPS* : Strata ou certificat d'entreprise | Le cadenas ; `http://` renvoie vers `https://` |
| 1.5 | `sudo reboot` | Le Core, ses applis, ses entrées et sorties reviennent seuls |

## 2. Cartes DeckLink

| | Faire | Attendu |
| --- | --- | --- |
| 2.1 | *Matériel › Cartes SDI* | Chaque carte avec son modèle, ses connecteurs ; version de Desktop Video |
| 2.2 | Connecteurs en entrée ; brancher une source 1080i50 puis 1080p50, 720p50, 2160p50 (carte 12G) | Le signal et son format suivis d'eux-mêmes, aperçu juste, timecode de la source |
| 2.3 | Débrancher, rebrancher une entrée | « Pas de signal », puis l'image revient seule ; charge du processeur basse sans signal |
| 2.4 | Source avec son embarqué (8 ou 16 canaux) ; `test level sdi/<carte>/<n>` | Le niveau des canaux de la source, le bon nombre de canaux |
| 2.5 | Connecteur en sortie ; `test bars sdi/<carte>/<n>` ; regarder sur un moniteur avec afficheur audio | La mire, 10 bits ; « avec le son » sur la tuile ; -21 RMS sur les canaux 1-2, rien ailleurs ; l'éclair et le trou du son ensemble |
| 2.6 | Couper la commande | Noir sur la sortie au bout d'une seconde, silence |
| 2.7 | Référence (black burst ou tri-level) sur la carte ; *Genlock* activé, décalage réglé | « REF verrouillée », « sorties calées dessus » ; sortie verrouillée sur un oscilloscope ; le décalage déplace bien la phase |
| 2.8 | Quatre entrées et deux sorties en même temps, 10 min | Aucune image perdue (compteurs), processeur raisonnable (*Santé › Ressources*) |

## 3. Horloge

| | Faire | Attendu |
| --- | --- | --- |
| 3.1 | *Matériel › Horloge* : source PTP, domaine du grand maître, interface PTP | ptp4l verrouillé (écart < 1 µs), chrony le suit |
| 3.2 | PTP coupé | Repli NTP, une alerte |
| 3.3 | Timecode maison | Égal à celui d'un appareil verrouillé sur le même PTP (ST 2059) |

## 4. ST 2110 (Intel E810)

| | Faire | Attendu |
| --- | --- | --- |
| 4.1 | *Matériel › Réseau* : rôles des ports (média 1, média 2, PTP, gestion) | Les ports E810 avec leur vitesse (25/100G) et leur lien |
| 4.2 | Flux reçus et envoyés par le Core : **Noyau** | Un flux reçu (2110-20 1080i50 d'un appareil de la régie) : image dans l'aperçu, 0 paquet perdu |
| 4.3 | Même chose en **AF_XDP**, puis **DPDK** (IOMMU actif, adresses des ports données, redémarrage du Core demandé) | Idem ; en DPDK, les ports média quittent le noyau (`ip link` ne les montre plus), le port de gestion jamais |
| 4.4 | Flux en 2022-7 (rouge + bleu) ; débrancher le câble rouge, puis le rebrancher et débrancher le bleu | Aucune image perdue ; l'état montre la patte coupée |
| 4.5 | Flux vidéo envoyé ; `test bars 2110/<flux>` ; le recevoir sur un appareil ou un analyseur | Image juste, cadencement 2110-21 accepté (pas d'erreur de buffer côté récepteur) |
| 4.6 | Flux de son envoyé (L24 1 ms, 8 canaux) ; `test tone 2110/<flux> --channels 8` | Reçu par un appareil AES67/2110-30 à -21 RMS |
| 4.7 | Flux de son reçu ; `test level 2110/<flux>` | Le niveau de la source |
| 4.8 | *ST 2110 › Ports média* | Débit mesuré proche de ce que demandent les flux ; place restante crédible ; alerte au-delà de 90 % |
| 4.9 | Couper un flux, en ajouter un | Les autres flux ne bougent pas (pas d'image perdue) |

## 5. NMOS avec Bitfocus Buttons

| | Faire | Attendu |
| --- | --- | --- |
| 5.1 | *Matériel › Réseau › NMOS* activé ; Buttons sur le même réseau | Buttons voit le serveur (découverte mDNS, sans registre) |
| 5.2 | Si Buttons ne le voit pas | **À me signaler** : s'il ne sait lire qu'un registre (IS-04 Query API), j'ajoute l'inscription à un registre, en option |
| 5.3 | Brancher un récepteur du Core sur un émetteur depuis Buttons | Le flux reçu change ; badge « NMOS » dans *ST 2110* ; les autres flux ne bougent pas |
| 5.4 | Couper / rallumer un émetteur du Core depuis Buttons | Le flux s'arrête puis repart |
| 5.5 | Contrôleurs autorisés : mettre l'adresse de Buttons seule | Buttons continue ; une autre machine reçoit un refus |

## 6. GPU

| | Faire | Attendu |
| --- | --- | --- |
| 6.1 | *Matériel › GPU* | Modèle, pilote, encodage et décodage (NVENC/NVDEC ou VA-API) |
| 6.2 | Une sortie réseau encodée (H.264, encodeur automatique) | L'encodeur matériel est choisi ; charge du GPU dans *Santé* |

## 7. Flux réseau et NDI

| | Faire | Attendu |
| --- | --- | --- |
| 7.1 | Entrée SRT (ou RTMP, HLS, RTSP) | Image et son dans l'aperçu ; reprise seule après une coupure de la source |
| 7.2 | Sortie SRT encodée d'une entrée ; la lire avec VLC ou ffplay | Image et son ; noir et silence quand rien n'arrive |
| 7.3 | NDI : sources de la régie listées | Les sources NDI visibles |

## 8. Santé, alertes, SNMP

| | Faire | Attendu |
| --- | --- | --- |
| 8.1 | *Santé › Alertes* : webhook ou e-mail ; débrancher une entrée SDI | Une alerte, puis sa fin au rebranchement |
| 8.2 | SNMP activé ; `snmpwalk -v2c -c <communauté> <serveur>` | État du serveur, des applis, alertes |

## 9. Applis et mises à jour

| | Faire | Attendu |
| --- | --- | --- |
| 9.1 | *Bibliothèque* : installer une appli depuis GitHub | Installée, activée, ouverte depuis le cockpit |
| 9.2 | L'appli lit une entrée SDI et écrit une sortie (MXL) | Image et son de bout en bout |
| 9.3 | *Mises à jour* : mettre à jour le Core | Redémarrage, nouvelle version ; entrées et sorties reviennent |
| 9.4 | Couper le courant pendant 30 s, rallumer | Tout revient sans intervention |

## Ce qu'il me faut en retour

Les numéros qui ne passent pas, avec : ce qu'on a vu, le modèle exact du matériel concerné, et
l'export des journaux. Pour le ST 2110, en plus : `ethtool -i <port>` et la sortie de
*ST 2110 › Ports média*.

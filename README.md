# Strata Core

Le socle des serveurs Strata. Sa page web est le cockpit du serveur : on y ouvre les applis Strata actives (Graphics, LiveCut, Multiview, Network, Media, Call), et les administrateurs y règlent le serveur : comptes, matériel, santé, mises à jour, autres serveurs.

## Prérequis

**Serveur**
- Ubuntu Server 24.04 LTS, 64 bits (x86-64)
- 4 cœurs et 8 Go de mémoire conseillés, 20 Go libres (la configuration de chaque appli s'y ajoute)
- Un compte avec `sudo`
- Un accès internet pendant l'installation (sinon : l'archive du Core, voir [docs/INSTALLATION.md](docs/INSTALLATION.md))

**Réseau**
- Un nom pour le serveur, et le même nom précédé de `*.` dans le DNS de l'entreprise, par exemple `strata01.regie.exemple.fr` et `*.strata01.regie.exemple.fr`. Le cockpit répond au nom du serveur, chaque appli à son sous-domaine (`livecut.strata01.regie.exemple.fr`). L'installateur vérifie le nom et donne les enregistrements à créer.
- Sans DNS, sur un petit réseau : le nom en `.local` (par exemple `strata01.local`) fonctionne, mais les sous-domaines sont mal reconnus par Windows.

**Selon le matériel**
- **Cartes Blackmagic** (DeckLink, UltraStudio, Intensity) : l'archive du pilote **Desktop Video pour Linux**, téléchargée sur [blackmagicdesign.com/fr/support](https://www.blackmagicdesign.com/fr/support/family/capture-and-playback) (Blackmagic demande ses coordonnées). L'installateur explique comment la copier sur le serveur et l'attend.
- **NDI** : rien à préparer. L'installateur télécharge le SDK NDI ; sa licence est à accepter pendant l'installation.
- **GPU** (NVIDIA, Intel, AMD) : rien à préparer. L'installateur pose le pilote NVIDIA ou VA-API.
- **Secure Boot** actif : au redémarrage qui suit, il faut enrôler la clé MOK (écran bleu « Perform MOK management ») pour que les pilotes Blackmagic et NVIDIA se chargent.

## Installation

Sur le serveur, en SSH :

```bash
curl -fsSL https://github.com/DamienP2/Strata-Core/releases/latest/download/install.sh | sudo bash
```

L'installateur s'ouvre dans le terminal et guide pas à pas :

1. vérification du serveur ;
2. nom du serveur (nom d'usage, nom DNS, fuseau) ;
3. applis à installer et activer ;
4. matériel : cartes Blackmagic, NDI, GPU ;
5. pare-feu ;
6. compte administrateur ;
7. récapitulatif, puis l'installation, étape par étape.

À la fin, il affiche l'adresse du cockpit, par exemple `http://strata01.regie.exemple.fr`, et ce qui reste à faire (enregistrement DNS, redémarrage pour un pilote).

Sans écran, pour automatiser, et ce qui est posé sur le serveur : [docs/INSTALLATION.md](docs/INSTALLATION.md).

## Après l'installation

- **Ouvrir une appli** : depuis le cockpit.
- **Activer ou ajouter une appli** : *Réglages › Bibliothèque* (administrateurs).
- **HTTPS, nom, adresses** : *Réglages › Ce serveur*.
- **Mises à jour** du Core et des applis : *Réglages › Ce serveur › Mises à jour*.
- **Plusieurs serveurs, un seul cockpit** : *Réglages › Ce serveur › Serveurs*.
- **Matériel** (cartes SDI, NDI, ST 2110, réseau, GPU, horloge) et **santé** (ressources, journal, alertes, SNMP) : dans les réglages.
- **Ajouter une carte, NDI, un pilote GPU, réparer, désinstaller** : relancer la commande d'installation.

## Développement

[docs/DEVELOPPEMENT.md](docs/DEVELOPPEMENT.md) ; le contrat des applis : [docs/APPLIS.md](docs/APPLIS.md) ; les releases : [docs/RELEASES.md](docs/RELEASES.md).

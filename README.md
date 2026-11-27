# Strata Core

Le socle des serveurs Strata. Sa page web est le cockpit du serveur : on y ouvre les applis Strata actives (Graphics, LiveCut, Multiview, Network, Media, Call), et les administrateurs y règlent le serveur.

> En préparation : l'installateur décrit ci-dessous n'est pas encore disponible.

## Prérequis

**Serveur**
- Ubuntu Server 24.04 LTS, 64 bits (x86-64)
- Configuration minimale : en cours de calcul (Core seul, puis ce qu'ajoute chaque appli)
- Un compte avec `sudo`
- Un accès internet pendant l'installation

**Réseau**
- Un nom pour le serveur, et le même nom précédé de `*.` dans le DNS de l'entreprise, par exemple `strata01.regie.exemple.fr` et `*.strata01.regie.exemple.fr`. Le cockpit répond au nom du serveur, chaque appli à son sous-domaine (`livecut.strata01.regie.exemple.fr`).
- Sans DNS, sur un petit réseau : le nom en `.local` (par exemple `strata01.local`) fonctionne, mais les sous-domaines sont mal reconnus par Windows.

**Selon le matériel**
- **Cartes Blackmagic** (DeckLink, UltraStudio, Intensity) : l'archive du pilote **Desktop Video pour Linux**, téléchargée sur [blackmagicdesign.com/fr/support](https://www.blackmagicdesign.com/fr/support/family/capture-and-playback) (Blackmagic demande ses coordonnées). L'installateur explique comment la copier sur le serveur.
- **NDI** : rien à préparer. L'installateur télécharge le SDK NDI ; sa licence est à accepter pendant l'installation.

## Installation

Sur le serveur, en SSH :

```bash
curl -fsSL https://raw.githubusercontent.com/DamienP2/Strata-Core/HEAD/install.sh | sudo bash
```

L'installateur s'ouvre dans le terminal et guide pas à pas :

1. vérification du serveur ;
2. nom du serveur ;
3. choix des applis à installer, et de celles à activer tout de suite ;
4. cartes Blackmagic et NDI ;
5. dossier des données ;
6. compte administrateur.

À la fin, il affiche l'adresse du cockpit, par exemple `https://strata01.regie.exemple.fr`.

## Après l'installation

- **Ouvrir une appli** : depuis le cockpit.
- **Activer ou ajouter une appli** : *Bibliothèque*, dans les réglages du cockpit (administrateurs).
- **Mises à jour** : *Serveur › Mises à jour*, dans les réglages du cockpit.
- **Ajouter une carte, NDI, désinstaller** : relancer la commande d'installation.

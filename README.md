# Strata Core

Le socle des serveurs Strata. Il gère le serveur (utilisateurs, matériel, santé, mises à jour) et les applis Strata qu'on y installe : Graphics, LiveCut, Multiview, Network, Media, Call.

> En préparation : l'installateur décrit ci-dessous n'est pas encore disponible.

## Prérequis

**Serveur**
- Ubuntu Server 24.04 LTS, 64 bits (x86-64)
- 4 cœurs et 8 Go de mémoire au minimum ; davantage selon les applis activées
- 20 Go libres pour le système et les applis, plus l'espace des médias
- Un compte avec `sudo`
- Un accès internet pendant l'installation

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
2. choix des applis à installer, et de celles à activer tout de suite ;
3. cartes Blackmagic et NDI ;
4. port, dossier des données ;
5. compte administrateur.

À la fin, il affiche l'adresse de l'interface web, par exemple `http://192.168.1.20:8080`.

## Après l'installation

- **Applis** : *Bibliothèque*, dans l'interface web. Une appli installée peut rester inactive ; on l'active quand on en a besoin.
- **Mises à jour** : *Serveur › Mises à jour*, dans l'interface web.
- **Ajouter une carte, NDI, désinstaller** : relancer la commande d'installation.

# Patchbay

## Produit

Patchbay est une matrice de routage audio locale pour macOS, inspirée des usages de Loopback. Le but est de choisir précisément le son qui entre, de le mixer canal par canal et de l'envoyer vers un périphérique virtuel, une ou plusieurs écoutes physiques, ou les deux. Le câblage doit rendre lisible le chemin du signal, de la source au périphérique destinataire.

- Application autonome en **Rust et Tauri 2**, avec **Vue 3** pour l'interface et l'état métier, et **React Flow** pour le graphe embarqué dans Vue. Le signal audio est traité côté Rust.
- Interface en français et identité visuelle propre à Patchbay. Employer les termes établis dans l'app, dont **Canaux de sortie**, **Bus interne**, **Interfaces virtuelles** et **Écoute physique**.
- Cible configurée macOS 14.2 ou ultérieur. Le bundle local a été exécuté sur Apple Silicon. Qualifier les fonctions natives par version de macOS et par matériel avant d'annoncer leur compatibilité.
- La capture Core Audio par application reprend une partie du travail de TalkTrack. Conserver son attribution et sa [licence MIT](LICENSE-TALKTRACK).

## Fonctionnalités de la série 0.2

### Sources, canaux et destinations

- Sources disponibles dans le graphe. Entrées Core Audio physiques et virtuelles, capture du son système, capture stéréo d'une application par identifiant de bundle, et tonalité de test. La capture par application tient compte des processus auxiliaires rattachés au bundle.
- Une source **Application** câblée coupe par défaut sa sortie directe pendant la capture pour faire passer son audio par le graphe. L'inspecteur permet de conserver la sortie directe en désactivant **Couper la sortie directe** avant de démarrer. Enregistrer ce choix avec la source. Appliquer la coupure uniquement pendant la lecture du tap de cette application. [Mode de lecture du tap Core Audio](https://developer.apple.com/documentation/coreaudio/catapmutebehavior)
- Les **Canaux de sortie** forment un étage de mixage entre les sources et les destinations. Un groupe possède de 1 à 64 canaux numérotés. Chaque canal somme ses entrées, affiche son niveau et peut alimenter plusieurs départs du même mix. Chaque départ garde son gain et son mute propres. Les câbles directs entre source et périphérique restent utilisables.
- **Insérer avant une sortie** reprend les liaisons existantes avec leurs identifiants, gains et mutes, puis crée un départ à gain unitaire vers chacun des canaux utilisés de cette sortie. Une réduction du nombre de canaux doit préserver les canaux encore câblés.
- Les destinations sont des sorties Core Audio, physiques ou virtuelles. Un périphérique physique sert au monitoring. Un périphérique virtuel peut être sélectionné comme entrée dans Ableton Live, OBS, Max ou une autre application.
- Pour un périphérique audio sélectionné, proposer les canaux d'entrée ou de sortie annoncés par Core Audio pour ce périphérique. Afficher la capacité de chaque direction et rendre tous ses canaux accessibles dans le graphe et le dialogue de câblage. Un périphérique agrégé utilise sa propre capacité. Si un périphérique disparaît ou expose moins de canaux, conserver les câbles enregistrés et empêcher les nouveaux câbles hors capacité.
- Un câble relie un canal à un autre. Plusieurs sources peuvent contribuer au même canal et une source peut être distribuée à plusieurs destinations. Le gain et le mute s'ajustent à chaud. La modification du câblage et de la fréquence du graphe se fait moteur arrêté.
- Le moteur valide les ports, les capacités des périphériques et les boucles des bus virtuels au niveau du canal. Un bus nommé Patchbay et son BlackHole sous-jacent ont la même identité pour la détection de boucle.
- Limites actuelles du graphe de 64 blocs, 1 024 câbles et 64 canaux par bloc. Fréquences de projet de 44,1, 48 et 96 kHz. Le moteur conserve la fréquence native des périphériques et rééchantillonne entre les horloges.

### Mixer des canaux de sortie

- La sélection d'un bloc **Canaux de sortie** affiche son mixer dans l'éditeur. Chaque canal a un fader de gain en dB, un mute et un vumètre de crête en dBFS avec son indication d'écrêtage. Tous les canaux du groupe doivent rester accessibles jusqu'à 64, y compris dans la fenêtre minimale.
- Appliquer le fader après la somme des sources du canal et avant l'envoi vers les destinations. Le vumètre affiche le signal après fader. Chaque câble de départ garde son propre gain et son propre mute.
- Le gain et le mute du canal s'ajustent pendant le routage. Enregistrer les réglages par canal dans le graphe. Les graphes enregistrés avant le mixer démarrent avec des faders à 0 dB et des canaux ouverts. Un agrandissement du groupe initialise les nouveaux canaux de la même façon.
- Deux canaux du même groupe peuvent former une paire stéréo sauvegardée avec le patch. Ils partagent un fader et un mute, et gardent leurs deux vumètres. Lier ou délier conserve les réglages actuels. Un mouvement du fader commun applique le même gain aux deux canaux dans une seule mise à jour, y compris pendant le routage. Chaque canal appartient à une seule paire à la fois.
- Chaque groupe propose deux prises supplémentaires, **Master G** et **Master D**, pour envoyer le mix stéréo complet vers les destinations choisies. Le master somme les pistes après leurs faders et leurs mutes. Les paires stéréo alimentent leurs côtés respectifs et les pistes mono sont centrées à −3 dB par côté.
- Le master a un fader et un mute propres, ainsi que deux vumètres après fader. Enregistrer son réglage avec le groupe, avec 0 dB et son ouvert par défaut. Les départs des canaux numérotés gardent leur réglage propre. Les prises Master gardent leur identité lors du redimensionnement du groupe.

### Bus internes, interfaces virtuelles et écoute

- **Ajouter un bus** crée un **Bus interne** nommé de 1 à 64 canaux, avec une entrée, une sortie et un vumètre par canal. Il somme les câbles entrants et distribue le signal à un autre bus, aux canaux de sortie ou à une écoute. Son nom et sa capacité sont enregistrés avec le patch.
- Traiter chaque bus interne au sein de la même trame et à la fréquence du graphe. Exécuter les étages internes dans l'ordre de leurs dépendances, sans tampon ou rééchantillonnage entre les étages. Vérifier qu'une chaîne de bus et un trajet direct avec les mêmes gains rendent l'impulsion au même échantillon. Refuser les boucles entre étages internes.
- **Interfaces virtuelles** publie des périphériques Core Audio duplex autonomes via le pilote HAL Patchbay. Chaque interface a son UID et ses propres flux. Elle figure parmi les sources et les destinations de Patchbay et dans les périphériques macOS. La capacité d'un bus interne ou d'un groupe de mixage est indépendante de celle du périphérique publié.
- Pour écouter un mix dans Patchbay, câbler le mix ou son bus interne à une sortie physique. Pour qualifier un parcours qui écrit puis relit une interface virtuelle, vérifier séparément les deux flux Core Audio.
- Le vumètre d'une sortie mesure le signal calculé dans le moteur. Pour confirmer la lecture réelle, mesurer le PCM relu côté entrée ou écouter la sortie physique sur le bon canal. En cas de silence, vérifier séparément le mix, l'écriture et la relecture du périphérique, l'autorisation macOS de lire l'entrée et la destination physique.
- Une entrée Core Audio, physique ou virtuelle, passe par l'autorisation macOS **Microphone**. Vérifier `NSMicrophoneUsageDescription` dans le bundle et `com.apple.security.device.audio-input` dans sa signature Hardened Runtime. Les taps système et application utilisent la permission de capture de l'audio système. [Apple, autoriser la capture](https://developer.apple.com/documentation/avfoundation/requesting-authorization-to-capture-and-save-media)
- « Navigateur entier » ou une application sélectionnée se capture côté application. Le choix de la sortie par défaut de macOS est un réglage distinct, utile notamment pour envoyer le son global vers une entrée virtuelle.

### Sessions et fonctionnement

- Plusieurs graphes nommés peuvent être enregistrés localement. Le moteur exécute la configuration active après une action explicite de démarrage.
- Le fichier `~/Library/Application Support/fr.patchbay.audio/session.json` utilise le schéma v1 et une écriture atomique. Préserver la lecture des graphes existants et leurs liaisons directes. Tester les évolutions de format sur un document isolé.
- Traiter l'audio en mémoire, dans des files bornées. Garder les opérations bloquantes, le stockage et les appels à l'interface hors des callbacks audio. Exposer les niveaux, l'écrêtage, les interruptions et les compteurs du moteur utiles au diagnostic.
- Les changements d'entrée et de destination audio viennent d'une action de l'utilisateur. Le routage doit fonctionner avec les périphériques sélectionnés dans le graphe et respecter les autorisations macOS requises.
- Charger le catalogue à partir des métadonnées Core Audio. Réserver l'ouverture des AudioUnits à la préparation des flux effectivement sélectionnés pour le routage.

## Périphériques virtuels natifs

- Installer une fois le pilote `Patchbay.driver` inclus dans le bundle macOS. Il publie jusqu'à 16 interfaces duplex à 48 kHz. Choisir indépendamment entre 1 et 64 entrées et entre 1 et 64 sorties par interface. Core Audio conserve leurs noms, UIDs et capacités dans le stockage du pilote. [Apple, créer un pilote audio virtuel](https://developer.apple.com/documentation/coreaudio/creating-an-audio-server-driver-plug-in)
- Les canaux communs se correspondent par numéro entre la sortie et l'entrée de l'interface. Les entrées au-delà du nombre de sorties restent silencieuses. Les sorties au-delà du nombre d'entrées restent disponibles comme ports Core Audio.
- Créer et supprimer les interfaces à chaud. La création, la suppression et la sélection d'un périphérique Patchbay respectent les périphériques par défaut du système. Si le pilote est installé et attend d'être chargé, **Interfaces virtuelles** propose **Relancer Core Audio** avec une confirmation explicite de la coupure audio et l'autorisation administrateur macOS. Arrêter le routage Patchbay et mettre les lectures et le monitoring en pause avant la relance. Après la relance, vérifier le chargement du pilote avant d'afficher le formulaire de création.
- Les anciens patchs qui référencent un agrégat BlackHole restent lisibles. Dans l'inspecteur, réaffecter explicitement les blocs de l'ancienne interface à une interface native compatible. Conserver les câbles, les canaux et leurs réglages.
- Pour la recette, créer deux interfaces aux capacités asymétriques, leur envoyer des signaux distincts et mesurer chaque retour PCM. Vérifier les ports annoncés dans les deux directions, l'isolation des interfaces et les entrées sans sortie correspondante, puis retrouver les mêmes affectations après rechargement.

## Fonctionnalités demandées pour les navigateurs

### Navigateur entier ou onglets choisis

Lorsqu'on ajoute **Brave, Chrome, Edge ou Safari** comme source, proposer deux modes.

1. **Navigateur entier** regroupe le son de ce navigateur dans une seule source. Il doit pouvoir être utilisé sans sélectionner chaque onglet.
2. **Onglets sélectionnés** présente les onglets capturables du navigateur. Chaque onglet sélectionné crée **un bloc d'entrée audio indépendant** dans Patchbay. Par exemple, sélectionner une visio et un lecteur dans deux onglets crée deux entrées avec leurs propres vumètres et câbles. Chacune peut être envoyée vers les canaux de sortie et les écoutes de son choix.

Le choix du navigateur et des onglets appartient à l'utilisateur. Le titre peut servir au libellé. Pour lier le signal au graphe, utiliser une identité d'onglet propre à son navigateur et à son profil. Distinguer cette identité de l'identifiant persistant du bloc. Une fermeture, un refus ou une révocation doit rendre le bloc silencieux avec un état explicite. La reprise d'une session doit retrouver le même onglet autorisé ou demander une nouvelle sélection. Tester le changement de titre, la navigation, les onglets de même nom et la perte de la connexion au navigateur.

Depuis la 0.2.6, **Ajouter un onglet** propose les captures ouvertes par l'extension `browser-extension/chromium`, incluse dans le bundle. L'utilisateur associe l'extension et l'app avec un code de session, puis clique sur l'action Patchbay dans chaque onglet voulu. L'extension transmet le PCM stéréo à 48 kHz au moteur Rust sur `127.0.0.1`. Le serveur vérifie l'origine de l'extension et le code d'association, et les files PCM restent bornées. Un bloc sauvegardé retrouve son onglet si l'extension conserve son identifiant. Pour un onglet remplacé, l'inspecteur permet de réaffecter le bloc et de retrouver ses câbles.

Le mode entier utilise la capture d'application Core Audio. La capture par onglet cible Chrome, Brave et Edge sur la base de `tabCapture`. Vérifier séparément les versions et le comportement de l'écoute de chaque navigateur. La capture individuelle dans Safari reste à qualifier avec les mécanismes pris en charge par Safari. [Chrome](https://developer.chrome.com/docs/extensions/reference/api/tabCapture) · [Microsoft Edge](https://learn.microsoft.com/en-us/microsoft-edge/extensions/developer-guide/api-support) · [Safari Web Extensions](https://developer.apple.com/documentation/safariservices/safari-web-extensions)

### Pistes individuelles d'une visio

La fonctionnalité avancée consiste à détecter les pistes audio distinctes disponibles dans une visio, puis à permettre de choisir lesquelles deviennent des entrées indépendantes. Exemple cible sur un BlackHole 64ch installé.

| Source de la visio | Destination dans le graphe |
| --- | --- |
| Participant 1 | BlackHole 64ch, canal 1 |
| Participant 2 | BlackHole 64ch, canal 2 |
| Participant 3 | BlackHole 64ch, canal 3 |

- Une piste disponible devient sélectionnable individuellement. Le routage vers un canal, le mixage avec d'autres sources et le monitoring restent libres. L'identité du participant s'affiche quand le service la fournit ou quand l'utilisateur l'associe. À défaut, fournir un libellé de piste que l'utilisateur peut renommer.
- Distinguer onglet, piste média et personne. Une piste reçue via WebRTC peut transporter une personne, un partage audio ou un mix. Si la visio ne fournit qu'une piste mixée, elle reste disponible comme un seul signal. L'isolation de voix à partir d'un mix constituerait un autre sujet de recherche.
- Vérifier les arrivées, départs et remplacements de pistes sur les visios compatibles. Garder les affectations du graphe liées à l'identité validée de la source et réserver la confirmation à une réaffectation réelle.
- Quand plusieurs niveaux se recouvrent, par exemple un onglet entier et ses pistes individuelles, rendre visible ce que chaque bloc contient. Le mélange de ces blocs doit venir des câbles choisis explicitement.
- La capture avancée porte sur l'audio autorisé dans le navigateur. Le traitement et le transport vers Patchbay restent locaux. Les choix de capture et les états de permission doivent être visibles pendant la session. [Spécification WebRTC](https://www.w3.org/TR/webrtc/) · [Spécification Media Capture and Streams](https://www.w3.org/TR/mediacapture-streams/)

### Critères de recette

- **Navigateur entier**. Produire des signaux connus dans deux onglets du même navigateur et retrouver les deux sur la source de ce navigateur.
- **Onglets**. Choisir deux onglets qui jouent des signaux distincts, vérifier les deux entrées et leur séparation PCM. Un troisième onglet joué en parallèle ne doit pas apparaître dans le mix des onglets choisis. Vérifier l'écoute locale et les cycles d'activation, arrêt et fermeture.
- **Visio**. Commencer par une page de test avec trois pistes distinctes et des signaux connus. Vérifier séparément les canaux 1, 2 et 3 du périphérique multicanal, les gains et mutes indépendants et l'absence de signal sur les canaux inutilisés. Qualifier ensuite le même parcours par navigateur et service de visio. Inclure le cas d'une seule piste mixée.
- Pour chaque preuve, indiquer le navigateur, sa version, la version de macOS, le mode de capture et le périphérique. Les tests de graphes et l'aperçu web vérifient l'interface et le modèle. La recette audio repose sur des mesures PCM ou une écoute contrôlée dans le bundle natif.

## Points d'entrée du code

- `src/App.vue`, `src/model.ts` et `src/bridge.ts` pour les workflows, le contrat du graphe côté interface et les appels Tauri.
- `src/GraphCanvas.vue` et `src/FlowCanvas.tsx` pour l'intégration de React Flow et le câblage. Garder des IDs distincts pour les prises d'entrée et de sortie. Recalculer les positions des prises lorsqu'un nombre de canaux change.
- `src-tauri/src/graph.rs` et `src-tauri/src/engine.rs` pour la validation du graphe, le mixage et les vumètres.
- `src-tauri/src/audio.rs`, `src-tauri/src/audio/macos_tap.rs` et `src-tauri/src/virtual_bus.rs` pour les périphériques, les captures Core Audio et la publication de bus.
- `src-tauri/src/main.rs` et `src-tauri/src/session.rs` pour les commandes et la session locale. Vérifier la cohérence des modèles Rust et TypeScript lors d'une évolution du graphe.

## Vérifier les changements

Utiliser Node 24.22 ou ultérieur, pnpm 11.25 via Corepack et Rust 1.97 ou ultérieur. Garder le travail local et préserver les configurations et réglages audio du poste pendant la recette. Utiliser des signaux synthétiques et des périphériques de test identifiés pour mesurer les régressions.

```sh
corepack pnpm install --frozen-lockfile
corepack pnpm typecheck
corepack pnpm test
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
corepack pnpm build:app
```

Pour l'interface, vérifier les tailles usuelles et la taille minimale de 880 × 600, les vrais gestes de câblage, l'inspecteur et la sauvegarde après rechargement. `corepack pnpm dev:web` ouvre un aperçu avec des données de démonstration. `corepack pnpm dev` lance l'application native. Le bundle local se trouve dans `src-tauri/target/release/bundle/macos/Patchbay.app`. Voir aussi le [README](README.md).

Pour le mixer, vérifier qu'il suit le bloc sélectionné, puis mesurer un changement de gain et un mute à chaud sur un canal qui alimente deux départs. Contrôler le vumètre après fader, les autres canaux et la persistance des réglages après rechargement.

Pour les paires stéréo, vérifier que la liaison et la dissociation conservent les réglages au moment du clic. Déplacer le fader commun et contrôler les deux niveaux séparément, ainsi qu'un troisième canal témoin. Vérifier la paire après rechargement du patch.

Pour le master, câbler **Master G** et **Master D** vers deux sorties distinctes et un canal numéroté vers une troisième. Vérifier le placement stéréo, le niveau des pistes mono, le fader du master, ses vumètres, la sauvegarde et la conservation des câbles Master au redimensionnement. Vérifier aussi la détection des boucles qui passent par un départ Master.

Pour un périphérique multicanal, comparer chaque direction au catalogue Core Audio et câbler le dernier canal annoncé. Vérifier séparément l'interface physique et son éventuel périphérique agrégé.

Pour la capture par application, jouer deux signaux distincts sur G/D vers un bus virtuel et câbler uniquement D dans Patchbay. Vérifier le PCM relu sur le bus, avec la sortie directe coupée puis conservée. Vérifier le retour de la sortie directe à l'arrêt du routage.

## Références à conserver

- [Radio.co, Loopback: Audio Routing for Mac Broadcasters](https://www.radio.co/blog/loopback-audio) pour le mix de plusieurs sources vers une application de diffusion.
- [Cycling '74, Software Overview: Loopback](https://cycling74.com/articles/software-overview-loopback) pour les usages de patch audio entre applications et avec Max.
- [Rogue Amoeba, Output Channels](https://rogueamoeba.com/support/manuals/loopback/?page=outputchannels) et [Monitors](https://rogueamoeba.com/support/manuals/loopback/?page=monitors) pour le comportement des canaux de sortie et des départs d'écoute.
- [Apple, Capturing system audio with Core Audio taps](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps) et [Existential Audio, BlackHole](https://github.com/ExistentialAudio/BlackHole) pour les mécanismes macOS et les variantes du pilote virtuel.

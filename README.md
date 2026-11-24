# Patchbay

Patchbay est une matrice de routage audio pour macOS. Vue pilote les graphes et les réglages, React Flow affiche les prises et les câbles, Rust traite le signal via Core Audio.

## Utiliser Patchbay

1. Ajoute tes sources audio. Tu peux prendre une entrée Core Audio, une application, le son système ou un signal de test.
2. Câble les sources vers un **Bus interne**, un groupe **Canaux de sortie** ou une écoute physique. Le bus interne est utile pour regrouper plusieurs sources et distribuer le mix à plusieurs destinations.
3. Démarre le routage. Le gain et le mute d’un câble sont réglables pendant la lecture. Arrête le moteur pour changer le câblage ou la fréquence du graphe. Les graphes sont enregistrés localement.

Avec **Ajouter un bus**, choisis un nom et 1 à 64 canaux. Par exemple, câble ton micro vers un bus **Voix**, puis envoie-le vers le mix principal et l'AudioFuse. Le bus est calculé dans la même trame que le reste du graphe, sans tampon supplémentaire. La latence totale dépend toujours des entrées et sorties du trajet.

Pour reprendre le câblage d’une sortie existante, ouvre **Canaux de sortie** puis choisis **Insérer avant une sortie**. Patchbay récupère ses câbles avec leurs réglages et relie chaque canal utilisé au même canal du périphérique.

Le Mac peut demander l’accès au microphone pour une entrée Core Audio, y compris BlackHole ou un bus Patchbay, et l’accès à l’audio système pour une capture d’application ou du système. Sur macOS 14 et 15, lance le son dans l’application cible avant le routage pour que Core Audio puisse l’identifier. [Documentation Apple sur les captures Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

Une source **Application** câblée passe par Patchbay pendant la capture. Sa sortie directe reprend à l'arrêt du routage. Pour écouter aussi la sortie d'origine pendant la capture, arrête le moteur, sélectionne le bloc de l'application et désactive **Couper la sortie directe** dans l'inspecteur.

### Récupérer un onglet du navigateur

1. Dans Patchbay, ouvre **Ajouter un onglet** puis **Configurer l'extension**. Copie le chemin du dossier de l'extension.
2. Dans Chrome, Brave ou Edge, ouvre la page des extensions, active le **Mode développeur**, puis choisis **Charger l'extension non empaquetée**. Sur macOS, utilise `⌘⇧G` dans le sélecteur pour coller le chemin fourni par Patchbay.
3. Génère le code d'association dans Patchbay. Colle-le dans les options de l'extension Patchbay et valide l'association.
4. Clique sur l'icône Patchbay dans chaque onglet à capturer. Retourne dans **Ajouter un onglet** et choisis les onglets autorisés. Patchbay crée une source stéréo par onglet.

Reclique sur l'icône dans l'onglet pour rendre le son à sa sortie habituelle. Si Patchbay redémarre, associe l'extension avec le nouveau code et réactive les captures. L'inspecteur d'un onglet en attente permet de choisir une nouvelle capture en conservant ses câbles. Pour le son global d'un navigateur, y compris Safari, utilise **Ajouter une source → Applications**.

### Envoyer le mix à une autre application

Dans **Interfaces virtuelles**, choisis un nom, le **Nombre d'entrées** et le **Nombre de sorties**, puis clique sur **Créer l'interface**. Chaque direction accepte de 1 à 64 canaux. Le pilote Patchbay crée un périphérique autonome à 48 kHz. Tu peux l'ajouter dans le graphe comme source, destination ou les deux. macOS et les applications comme Ableton Live ou OBS affichent les capacités propres à chaque direction.

Les canaux de même numéro sont reliés dans l'interface. Avec 8 entrées et 2 sorties, les sorties 1 et 2 alimentent les entrées 1 et 2, et les entrées 3 à 8 restent silencieuses.

Sur un nouveau Mac, installe le pilote signé `Patchbay.driver` du bundle dans `/Library/Audio/Plug-Ins/HAL`. Patchbay affiche son chemin sous **Interfaces virtuelles**. Si le pilote est installé et attend d'être chargé, arrête le routage, mets les lectures et le monitoring en pause, puis clique sur **Relancer Core Audio** et confirme. macOS peut demander une autorisation administrateur. Patchbay vérifie ensuite le chargement du pilote. Les créations d'interfaces qui suivent se font à chaud. [Guide Apple sur les pilotes HAL](https://developer.apple.com/documentation/coreaudio/creating-an-audio-server-driver-plug-in)

Pour reprendre un ancien patch avec un agrégat BlackHole, crée une interface native ayant assez de canaux, puis utilise **Réaffecter à** dans l'inspecteur du périphérique d'origine. Patchbay conserve le câblage et les niveaux. Le moteur vérifie les boucles au niveau des canaux de chaque interface.

## Développer

macOS 14.2 ou ultérieur, Xcode Command Line Tools, Rust 1.97+, Node 24.22+ et pnpm 11.25 via Corepack.

```sh
corepack pnpm install --frozen-lockfile
corepack pnpm dev
corepack pnpm typecheck
corepack pnpm test
corepack pnpm build:app
```

Le bundle local est généré dans `src-tauri/target/release/bundle/macos/Patchbay.app` et signé ad hoc. `corepack pnpm dev:web` ouvre l’aperçu du graphe dans le navigateur sur `127.0.0.1:1432`. Le routage natif passe par l’app Tauri.

Le fichier de session est stocké dans `~/Library/Application Support/fr.patchbay.audio/session.json`. La capture audio passe en mémoire et Patchbay n’écrit aucun enregistrement.

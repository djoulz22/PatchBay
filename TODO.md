# Améliorations de Patchbay

Demandes à réaliser. Les spécifications du projet sont dans [AGENTS.md](AGENTS.md).

## Priorité 1 : recommandations de l'audit du 24 novembre 2026

Traiter cette section **avant toutes les autres**, dans l'ordre P1 à P8. Elle reprend le plan d'action du rapport `PATCHBAY-AUDIT-2026-11-24` (version 0.2.9). Chaque tâche indique les fichiers à modifier, la mise en œuvre attendue, les tests et les critères d'acceptation. Les numéros de ligne cités correspondent à la 0.2.9 ; se repérer d'abord au nom de la fonction.

Règles communes à toutes les tâches :

- Ne pas modifier le format de `session.json`. Les champs ajoutés à `EngineStatus` sont des données d'exécution, jamais enregistrées.
- Garder les messages d'interface et d'erreur en français, dans le ton de ceux qui existent.
- Après chaque tâche, toutes les vérifications de la section « Vérifier les changements » d'AGENTS.md doivent passer, y compris `cargo clippy ... -D warnings`.
- Une tâche est terminée quand ses critères d'acceptation automatiques passent. Les recettes manuelles sur matériel restent à consigner : en indiquer le résultat, ou « non réalisée », dans la case de la tâche.

### - [x] P1. Donner une priorité temps réel au thread de mixage (constat C1, priorité élevée)

**Problème.** `RenderPlan::run` (`src-tauri/src/engine.rs`, vers la ligne 348) tourne sur un thread ordinaire nommé `patchbay-audio`, réveillé toutes les 5 ms par `thread::park_timeout`. Sous charge CPU, le réveil prend du retard, les files de sortie se vident et le compteur `underruns` augmente.

**Mise en œuvre.**

1. Dans `src-tauri/Cargo.toml`, ajouter sous `[target.'cfg(target_os = "macos")'.dependencies]` : `mach2 = "0.5"` et `libc = "0.2"`. Les deux crates figurent déjà dans `Cargo.lock`, en dépendances indirectes (mach2 0.5.0, libc 0.2.191). Mettre le lockfile à jour avec `cargo update -p mach2 -p libc --precise <version déjà présente>` si `--locked` refuse la compilation.
2. Créer `src-tauri/src/realtime.rs` et le déclarer dans `main.rs` (`mod realtime;`). Il expose :

   ```rust
   /// Passe le thread courant en politique Mach à contrainte de temps.
   pub fn promote_current_thread(period: Duration, computation: Duration) -> Result<(), String>
   ```

   - Sous macOS : récupérer le port du thread avec `libc::pthread_mach_thread_np(libc::pthread_self())`, sans `mach_thread_self`, qui créerait un droit de port à libérer. Lire `mach2::mach_time::mach_timebase_info` et convertir les nanosecondes en unités absolues avec `ns * denom / numer`. Remplir `mach2::thread_policy::thread_time_constraint_policy { period, computation, constraint: period, preemptible: 1 }`. Appeler `thread_policy_set(port, THREAD_TIME_CONSTRAINT_POLICY, &mut policy as *mut _ as thread_policy_t, THREAD_TIME_CONSTRAINT_POLICY_COUNT)`. Renvoyer `Err` si le code n'est pas `KERN_SUCCESS`, avec le message « Priorité temps réel refusée par macOS (code N). ». Chaque bloc `unsafe` porte un commentaire `// SAFETY:`, comme dans `macos_tap.rs`.
   - Sur les autres systèmes : renvoyer `Ok(())` sans rien faire.
3. Dans `Runtime::start` (`engine.rs`), juste après `RenderPlan::open` réussi et avant `plan.run(...)`, appeler `realtime::promote_current_thread(BLOCK_TIME, Duration::from_micros(1_500))`. En cas d'échec, **ne pas** arrêter le routage : mémoriser l'échec pour l'état du moteur.
4. Ajouter à `EngineStatus` le champ `pub realtime: bool`, sérialisé `realtime`, à `true` si la promotion a réussi. Côté interface :
   - dans `src/model.ts`, ajouter `realtime: boolean` à `EngineStatus` et `realtime: false` à `idleStatus` ;
   - dans `readStatus` (`src/App.vue`), inclure `next.realtime !== status.value.realtime` dans la condition qui met `status.value` à jour ;
   - dans le bloc `engine-counters` (vers la ligne 1004 d'`App.vue`), afficher quand `status.running && !status.realtime` la ligne « Priorité temps réel indisponible », avec l'icône `Activity` déjà utilisée. Ajouter cette condition au `v-if` du bloc.
5. Hors périmètre de cette tâche : le rattachement aux `os_workgroup` de Core Audio et le pilotage du mixage depuis le callback de sortie. Ne pas les implémenter maintenant.

**Tests.** Dans `realtime.rs`, ajouter un test `#[cfg(target_os = "macos")]` qui lance un thread, y appelle `promote_current_thread(Duration::from_millis(5), Duration::from_micros(1_500))` et vérifie `Ok(())`. Relire ensuite la politique avec `thread_policy_get` (`get_default = 0`) et vérifier que `period` vaut la valeur convertie.

**Critères d'acceptation.**

- `cargo test` et Clippy passent ; le nouveau test réussit sur macOS.
- Dans l'app, au démarrage d'un routage, `routing_status` renvoie `realtime: true`.
- Recette manuelle : router la tonalité de test vers une sortie physique pendant 60 s, avec une charge de `yes > /dev/null` par cœur CPU. Relever le compteur `underruns` avant et après la modification. Il doit être nul ou nettement plus bas qu'avant.

Le 24 novembre 2026, sur macOS 26.5.2 (Apple Silicon), les 29 tests Rust, Clippy strict, le contrôle TypeScript, l'installation figée en dossier isolé et le build macOS passent. Le test Mach relit une période de 5 ms. Le noyau ajuste le budget demandé de 1,5 ms à 2,5 ms sur ce Mac. Sur un graphe tonalité vers bus interne, la commande Tauri renvoie `realtime: true` pendant le routage et `realtime: false` après l'arrêt.

**Recette manuelle sur sortie physique sous charge non réalisée.** Aucune comparaison d'underruns avant/après sur matériel.

### - [x] P2. Isoler la perte d'un périphérique au lieu d'arrêter tout le routage (constat C2, exigence OPS-02)

**Problème.** `RenderPlan::check_streams` (`engine.rs`, vers la ligne 414) renvoie `Err` dès qu'une entrée ou une sortie signale une erreur : déconnexion, changement de fréquence ou flux interrompu (voir `DeviceStream::error` dans `audio.rs` et `SystemTap::check_alive` dans `macos_tap.rs`). `run` s'arrête alors et coupe tout le graphe.

**Comportement attendu.**

- Une extrémité en erreur passe **hors ligne**. Une source hors ligne produit du silence. Une sortie hors ligne n'est plus alimentée, mais le reste du graphe continue, et ses vumètres aussi.
- Le moteur tente de rouvrir chaque extrémité hors ligne au plus **une fois par seconde**. Il réutilise le même `GraphNode`, la même fréquence de projet et, pour un onglet, le même `BrowserCapture`. Les onglets navigateur ne passent jamais hors ligne, car leur `error()` renvoie `None`.
- Pour une entrée ou une sortie Core Audio, ne tenter la réouverture que si le périphérique est de nouveau présent avec la même fréquence nominale qu'à l'ouverture. Ajouter dans `audio.rs` la fonction `pub(crate) fn device_rate(id: &str) -> Option<u32>`, qui appelle `macos_tap::device_sample_rate` sous macOS et renvoie `None` ailleurs. Les taps d'application et système tentent directement `audio::open_input`.
- Si la fréquence du périphérique a changé, l'extrémité reste hors ligne. Le message d'erreur actuel (« la fréquence de … a changé ») reste visible et l'utilisateur doit redémarrer le routage.
- Le routage ne s'arrête que sur `stop`, ou si **toutes** les sorties du graphe sont hors ligne depuis plus de 10 s. Dans ce dernier cas, renvoyer « Toutes les sorties du graphe sont indisponibles. ».

**Mise en œuvre.**

1. `Source` : remplacer l'usage direct de `SourceSignal::Capture` par une variante supplémentaire `SourceSignal::Offline { since: Instant, last_attempt: Instant, reason: String }`. `read_frame` remplit `frame` de zéros pour cette variante. Conserver dans `Source` une copie du `GraphNode` (`node: GraphNode`), nécessaire à la réouverture.
2. `Destination` : remplacer `stream: AudioOutput` par `stream: Option<AudioOutput>` et ajouter `offline: Option<(Instant, Instant, String)>`. `write_frame` mesure toujours le vumètre mais n'écrit rien si `stream` vaut `None`. `report` ignore les compteurs d'une sortie sans flux.
3. `check_streams(&mut self)` ne renvoie plus `Result` pour les pannes d'extrémité. Il bascule les extrémités en erreur hors ligne, puis lance les tentatives de réouverture dues.
4. Réouverture hors du thread de mixage. `cpal::Stream` est `Send` sur macOS (cpal 0.17).
   - Ajouter dans `audio.rs` une assertion statique `const _: () = { const fn assert_send<T: Send>() {} assert_send::<AudioInput>(); assert_send::<AudioOutput>(); };`.
   - Si elle compile, créer au démarrage du moteur un thread `patchbay-recovery`. Il reçoit les demandes de réouverture par un `mpsc::Sender<RecoveryRequest>` et renvoie les `AudioInput` ou `AudioOutput` ouverts par un second canal. Le thread de mixage lit ce canal avec `try_recv` au début de chaque bloc, sans jamais bloquer.
   - Si l'assertion échoue uniquement à cause de `SystemTap` : ses champs sont des `AudioObjectID`, un `AudioDeviceIOProcID` (pointeur de fonction), un `Arc<FrameQueue>` et des scalaires. Ajouter `unsafe impl Send for SystemTap {}`, avec un commentaire `// SAFETY:` expliquant que ces objets HAL sont globaux au processus et que `Drop` n'appelle que des fonctions HAL utilisables depuis n'importe quel thread.
5. Une fois le flux rouvert : remplacer le signal par `SourceSignal::Capture(Box::new(Capture::new(input, sample_rate)))`, ou réinstaller la sortie, puis appeler `prime()` et réinitialiser `initial_underruns`. Retirer l'identifiant de la liste hors ligne.
6. Ajouter à `EngineStatus` le champ `pub offline: Vec<String>`, sérialisé `offline` et contenant les identifiants de nœuds hors ligne, triés. Pendant une panne d'extrémité, `error` vaut le message de la dernière panne : « Micro USB déconnectée : reprise automatique en attente. ». Il revient à `None` quand plus rien n'est hors ligne.
7. Côté interface :
   - ajouter `offline: string[]` à `EngineStatus` et `offline: []` à `idleStatus` dans `model.ts` ;
   - dans `readStatus`, comparer aussi `next.offline.join() !== status.value.offline.join()` ;
   - passer `status.offline` à `GraphCanvas` (nouvelle prop `offline: string[]`), puis à `FlowCanvas` ;
   - dans `FlowCanvas.tsx`, vers la ligne 247, calculer `available: deviceAvailable(node, catalog) && !offline.includes(node.id)`. L'indicateur et le libellé « Indisponible » existants s'appliquent alors sans autre changement.

**Tests** (`engine.rs`, sans matériel).

- Extraire la logique de décision dans une fonction pure, testable sans flux, par exemple `fn recovery_due(offline_since: Instant, last_attempt: Instant, now: Instant) -> bool` et `fn all_outputs_lost(...) -> bool`.
- Tester : pas de tentative avant 1 s ; une tentative par seconde ensuite ; l'arrêt n'intervient qu'après 10 s de perte de toutes les sorties, jamais si une seule sortie reste en ligne.
- Tester qu'une source `Offline` produit des zéros et que le mix des autres sources reste correct. S'inspirer de `an_internal_bus_can_run_and_report_levels_without_audio_devices`.

**Critères d'acceptation.**

- Tests et Clippy verts.
- Recette manuelle : router un micro USB et la tonalité vers une sortie physique, puis débrancher le micro. La tonalité continue, le bloc du micro est marqué indisponible et l'erreur indique la reprise en attente. Rebrancher : le signal du micro revient en moins de 3 s, sans redémarrer.
- Recette manuelle : débrancher la seule sortie. Le routage s'arrête après environ 10 s avec le message prévu.

Les tests Rust couvrent le silence d'une source hors ligne, la poursuite du mix, le délai de reprise, les changements de fréquence ou de canaux et le délai d'arrêt quand toutes les sorties sont perdues. Playwright vérifie l'indicateur et l'alerte, y compris après une deuxième panne simulée. **Recettes de débranchement et de reconnexion sur matériel non réalisées.**

### - [x] P3. Demander l'autorisation micro avant de démarrer le moteur (constat C3)

**Problème.** `macos_tap::authorize_microphone` (`src-tauri/src/audio/macos_tap.rs`, ligne 46) peut attendre la réponse de l'utilisateur jusqu'à 120 s. Il est appelé depuis `audio::open_input`, donc dans `Runtime::start`, pendant que `start_routing` (`main.rs`, ligne 48) garde le verrou `Routing`. Pendant ce temps, l'état affiché reste figé.

**Mise en œuvre.**

1. Découper `authorize_microphone` dans `macos_tap.rs` en deux fonctions :
   - `pub(super) fn microphone_authorized() -> Result<()>` : lit `authorizationStatusForMediaType` sans jamais ouvrir de dialogue. `Ok` si le statut vaut 3 (autorisé). Sinon, `Err` avec le message actuel « Autorise le microphone dans Réglages Système > Confidentialité et sécurité. ».
   - `pub(super) fn request_microphone() -> Result<()>` : reprend le comportement actuel, avec dialogue si le statut vaut 0 et attente bornée à 120 s.
2. Dans `audio.rs`, `open_input` appelle `microphone_authorized()` à la place de `authorize_microphone()`. Exposer `pub fn request_microphone() -> Result<(), String>`, qui appelle `macos_tap::request_microphone` sous macOS et renvoie `Ok(())` ailleurs.
3. Dans `main.rs`, ajouter la commande `authorize_audio_input`, qui exécute `audio::request_microphone` dans `tauri::async_runtime::spawn_blocking`, **sans** prendre le verrou `Routing`. L'enregistrer dans `generate_handler!`.
4. Dans `src/bridge.ts`, ajouter `export function authorizeAudioInput(): Promise<void> { return invoke('authorize_audio_input') }`.
5. Dans `toggleRouting` (`App.vue`, ligne 313), dans la branche de démarrage, entre `await persist()` et `await startRouting(project.value)` : si une liaison du projet a pour source un nœud `deviceInput`, appeler `await authorizeAudioInput()`. Une erreur à cet endroit passe par le `catch` existant et s'affiche avec `showError`.

**Tests.** Placer la condition dans une fonction de `model.ts`, `projectNeedsMicrophone(project: Project): boolean`, et l'utiliser dans `toggleRouting`. Elle renvoie `true` seulement si une route part d'un nœud `deviceInput`. Son test unitaire est écrit en P4, qui installe Vitest.

**Critères d'acceptation.**

- Tests et Clippy verts.
- Recette manuelle : réinitialiser l'autorisation avec `tccutil reset Microphone fr.patchbay.audio`, puis démarrer un graphe qui contient un micro. Le dialogue macOS s'affiche avant le démarrage du moteur, et l'état de l'interface reste mis à jour pendant l'attente. Un refus affiche le message d'autorisation sans démarrer le moteur.

En simulation, la demande précède le démarrage, l'interface continue de lire l'état et le refus bloque le démarrage. **Recette avec réinitialisation de l'autorisation macOS non réalisée.**

### - [x] P4. Tester la logique du graphe côté interface (constat C4)

**Problème.** Aucun test ne couvre `src/model.ts`, ni la logique de redimensionnement placée dans `App.vue` (`resizeMixStage`, ligne 476).

**Mise en œuvre.**

1. Installer Vitest : `corepack pnpm add -D vitest`. Si pnpm signale un conflit de dépendance avec Vite 7, utiliser `corepack pnpm add -D vitest@3.2`. Valider le `pnpm-lock.yaml` mis à jour.
2. Dans `package.json`, réorganiser les scripts :
   - `"test:web": "vitest run"`
   - `"test:rust": "cargo test --locked --manifest-path src-tauri/Cargo.toml"`
   - `"test:driver": "sh driver/test.sh"` (créé en P5 ; si P5 n'est pas encore fait, ne pas l'inclure dans `test`)
   - `"test": "pnpm run test:web && pnpm run test:rust && pnpm run test:driver"`
3. Ajouter `test: { environment: 'node', include: ['src/**/*.test.ts'] }` dans `vite.config.ts`, avec `/// <reference types="vitest/config" />` en tête du fichier.
4. Extraire de `resizeMixStage` (`App.vue`) une fonction pure dans `model.ts` :

   ```ts
   export function resizeMixStage(project: Project, catalog: AudioCatalog, nodeId: string, count: number): Project
   ```

   - Elle lève une `Error` avec exactement les messages actuels : « Choisis entre 1 et 64 canaux. », « Le canal N est encore relié. Déconnecte les canaux à retirer avant de réduire le groupe. » et « Ce redimensionnement fermerait une boucle audio par le Master. ».
   - Le minimum se calcule comme `minimumSelectedChannels` dans `App.vue` (ligne 124), à partir des routes du nœud.
   - `App.vue` appelle ensuite cette fonction, remet la valeur du champ et affiche l'erreur en cas d'exception. Le comportement visible ne change pas.
5. Créer `src/model.test.ts`, qui construit ses projets et catalogues à la main (s'inspirer de `starterSession` et de `emptyCatalog`) et couvre au minimum :
   - `insertOutputChannels` :
     - les routes existantes vers la sortie gardent leur `id`, `gain`, `muted` et leurs canaux, mais pointent vers le nouveau groupe ;
     - un départ à gain 1 est créé par canal utilisé, au même numéro ;
     - sans câble entrant, un départ est créé par canal, jusqu'à `min(kind.channels, canaux du périphérique)` ;
     - erreur si un canal utilisé dépasse la taille du groupe ;
     - erreur si la sortie reçoit déjà un groupe ;
     - erreur à 64 blocs ou au-delà de 1 024 liaisons.
   - `canInsertOutputChannels` : faux pour un nœud qui n'est pas une sortie, et pour une sortie déjà alimentée par un groupe.
   - `validConnection` : refus d'un doublon, d'une boucle entre étages et d'une boucle par interface virtuelle ; acceptation de `Master G` (canal 64) et `Master D` (canal 65) comme sources d'un groupe `outputChannels` uniquement.
   - `resizeMixStage` :
     - un agrandissement initialise les nouveaux canaux à 0 dB et sans mute ;
     - une réduction conserve les canaux câblés et retire les paires stéréo hors limites ;
     - une réduction sous un canal câblé est refusée avec le bon message ;
     - les routes Master (canaux 64 et 65) restent intactes.
   - `channelLevels`, `isMasterChannel` et `projectNeedsMicrophone` (P3).

**Critères d'acceptation.** `corepack pnpm test:web` passe, avec au moins un test par point ci-dessus, et `corepack pnpm typecheck` reste vert. L'interface se comporte comme avant : redimensionner un groupe câblé dans `corepack pnpm dev:web` produit les mêmes messages.

Les 26 tests Vitest et le contrôle TypeScript passent. Playwright confirme le message de réduction refusée, la restauration de la valeur et l'agrandissement d'un groupe câblé. Les sous-commandes de la suite passent par Corepack pour utiliser la version de pnpm du projet.

### - [x] P5. Intégrer le test du pilote HAL à la suite de tests (écart A3)

**Problème.** `driver/driver_test.c` n'est appelé nulle part. Il attend en argument le chemin d'un bundle compilé (`assert(argc == 2)`, ligne 91).

**Mise en œuvre.**

1. Créer `driver/test.sh` (POSIX `sh`, `set -eu`). Le script :
   - crée un dossier temporaire avec `mktemp -d` et le supprime à la sortie avec `trap` ;
   - compile le pilote en bundle instrumenté : `xcrun --sdk macosx clang -std=c11 -Wall -Wextra -Werror -O1 -g -fsanitize=address,undefined -mmacosx-version-min=14.2 -bundle -framework CoreFoundation driver.c properties.c -o "$tmp/Patchbay.bundle"` ;
   - compile le test avec les mêmes options, sans `-bundle` : `driver_test.c -framework CoreFoundation -o "$tmp/driver_test"` ;
   - exécute `"$tmp/driver_test" "$tmp/Patchbay.bundle"` ;
   - résout les chemins sources depuis le dossier du script, comme `build.sh`.
2. Ajouter le script `test:driver` dans `package.json` (voir P4).
3. Dans la section « Vérifier les changements » d'AGENTS.md et dans la section « Développer » du README, préciser que `corepack pnpm test` exécute les tests de l'interface, du moteur Rust et du pilote HAL sous AddressSanitizer et UndefinedBehaviorSanitizer.

**Critères d'acceptation.** `sh driver/test.sh` affiche « Deux interfaces isolées, 2 entrées / 8 sorties et 64 entrées / 2 sorties, PCM vérifié. » et se termine avec le code 0. `corepack pnpm test` l'exécute.

Le test du pilote passe dans `corepack pnpm test` sous AddressSanitizer et UndefinedBehaviorSanitizer et confirme l'isolation PCM des deux interfaces simulées.

### - [x] P6. Mettre à jour le comparatif avec Loopback et la documentation (écarts A1, A2, A5)

**Mise en œuvre.**

1. Dans `COMPARATIF-LOOPBACK.md`, remplacer « Patchbay 0.2.8 » par la version courante de `package.json` dans le titre, le verdict et la section « Périmètre et preuves ». Remplacer aussi le nombre de tests Rust par le résultat de `cargo test`, et ajouter les tests Vitest (P4) et le test du pilote (P5).
2. Remplacer **chaque** lien comportant une ancre de ligne (`#L123`) par un lien vers le fichier seul, suivi du nom de la fonction ou du composant en code. Par exemple, remplacer `[Moteur Patchbay](src-tauri/src/main.rs#L39)` par `` [Moteur Patchbay](src-tauri/src/main.rs) (`start_routing`) ``. Trouver la fonction visée en ouvrant le fichier à la ligne indiquée dans la version 0.2.8 ou 0.2.9. Vérifier ensuite qu'il ne reste aucune ancre avec `grep -n '#L[0-9]' COMPARATIF-LOOPBACK.md`.
3. Relire les verdicts touchés par P2 (OPS-02 : « Reprise automatique … absente » devient la reprise automatique des extrémités, en précisant que les agrégats modifiés à chaud ne sont pas couverts) et par P1 (OPS-05 : le diagnostic affiche la priorité temps réel). Ne changer un verdict que si le code le justifie, et recompter les totaux couvertes, partielles et absentes.
4. Dans AGENTS.md, section « Pistes individuelles d'une visio », remplacer l'exemple BlackHole 64ch par une interface virtuelle Patchbay : remplacer « sur un BlackHole 64ch installé » par « sur une interface Patchbay de 64 entrées », et `BlackHole 64ch, canal N` par `Interface Patchbay 64, canal N` dans le tableau.

**Critères d'acceptation.** Aucune ancre `#L` dans le comparatif, les version et nombres de tests correspondent aux sorties réelles des commandes, et les totaux par verdict correspondent au tableau.

Le comparatif 0.2.9 reprend les 35 tests Rust, les 26 tests Vitest et le test HAL sous ASan et UBSan. Les liens locaux sont vérifiés et les références de code nomment les symboles visés. Les 30 verdicts se répartissent en 4 couverts, 19 partiels et 7 absents. L'exemple de visio utilise une interface Patchbay de 64 entrées.

### - [x] P7. Reporter l'écrêtage du Master dans l'indicateur global (constat C5)

**Problème.** `Meter::observe_master` (`engine.rs`, ligne 58) met à jour `master.clipped_channels` mais pas `self.clipping`. Un écrêtage du seul Master laisse donc `clipping` à `false` pour le bloc.

**Mise en œuvre.** Dans `observe_master`, ajouter `self.clipping |= level > 1.0;` dans la boucle, après le calcul de `level`. Comme `master` emprunte `self.master` en mutable, calculer d'abord un booléen local `overload` dans la boucle, puis faire `self.clipping |= overload` après la boucle.

**Tests.** Dans `master_sums_post_fader_audio_and_feeds_independent_stereo_sends` (`engine.rs`), ou dans un nouveau test : alimenter un groupe dont les canaux restent sous 1,0 mais dont la somme Master dépasse 1,0, puis vérifier que `meter.clipping` vaut `true` et `meter.clipped_channels` reste à `false`.

**Critères d'acceptation.** Test vert, sans changement visible dans `OutputMixer.vue` ni `FlowCanvas.tsx`, qui lisent déjà `master.clippedChannels`.

Le test du mixeur reproduit l'indicateur global absent avant la correction. Après la correction, un écrêtage du Master déclenche `clipping`, avec les indicateurs de piste à `false` lorsque leurs signaux restent sous le seuil.

### - [x] P8. Fixer la version de Node du projet (constat C6)

**Problème.** Sur le poste de développement, `corepack pnpm typecheck` échoue : le Node actif est en v22 (celui de `~/.hermes`), alors que `package.json` exige `>=24.22.1`. pnpm refuse ensuite de réinstaller `node_modules` hors d'un terminal interactif (`ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`).

**Mise en œuvre.**

1. Créer `.node-version` contenant `24.22.1`, lu par fnm, nodenv, asdf et la plupart des gestionnaires de versions.
2. Dans le README, section « Développer », ajouter : « Vérifie `node --version` avant `corepack pnpm install`. Avec un Node plus ancien, pnpm refuse l'installation. ».
3. Ne pas modifier le `PATH` ni l'installation Node du poste. Si les commandes échouent pour cette raison pendant les tâches P1 à P7, lancer directement `./node_modules/.bin/vue-tsc --noEmit` et `./node_modules/.bin/vitest run`, puis le signaler dans le compte rendu.

**Critères d'acceptation.** `.node-version` et la note du README sont présents. Avec Node 24.22 ou ultérieur, `corepack pnpm install --frozen-lockfile && corepack pnpm typecheck && corepack pnpm test` passe.

L'installation figée, le contrôle TypeScript, la suite complète et le build macOS passent avec Node 24.22.1 et pnpm 11.25.0, en sélectionnant Node pour les commandes de recette.

## Sources iPhone et iPad

- [ ] **Micro de l'iPhone via Continuité**. Proposer le micro dans **Ajouter une source → Entrées audio** dès que macOS le publie. Réutiliser l'entrée Core Audio existante, avec le nombre de canaux annoncé par le périphérique. L'utilisateur choisit cette source dans le graphe, indépendamment du micro par défaut du Mac. [Apple, micro de l'iPhone sur Mac](https://support.apple.com/fr-fr/102546)
- [ ] **Son des apps de l'iPhone et de l'iPad via USB (IDAM)**. Brancher l'appareil en USB-C ou Lightning, lui faire approuver le Mac si demandé, puis cliquer sur **Activer** dans **Configuration audio et MIDI**. Sélectionner l'entrée apparue dans Patchbay. Le flux correspond à la sortie audio de l'appareil. L'utiliser comme entrée Core Audio et conserver ses canaux. [Apple, Configuration audio et MIDI](https://support.apple.com/guide/audio-midi-setup/set-up-audio-devices-ams59f301fda/mac)
  - **Activation dans Patchbay**. Pour intégrer ce parcours à **Ajouter une source**, tester le sélecteur macOS `CAInterDeviceAudioViewController`, qui permet à l'utilisateur de connecter un appareil USB compatible. [Apple, CoreAudioKit](https://developer.apple.com/documentation/coreaudiokit/cainterdeviceaudioviewcontroller)
  - **Recette des entrées Core Audio**. Tester la voix sur l'iPhone via Continuité et lire un signal stéréo connu sur chacun des deux appareils en USB. Contrôler le signal reçu, la séparation gauche/droite et le routage vers une interface virtuelle et une écoute physique. Vérifier l'indication d'indisponibilité à la déconnexion et la conservation du câblage à la reconnexion. Tester séparément l'usage simultané de Continuité et d'IDAM sur le même iPhone.
- [ ] **Micro de l'iPad**. Apple réserve cette entrée Continuité à l'iPhone. Pour obtenir le micro de l'iPad sur une source dédiée, prototyper une app compagnon iPadOS qui capture le micro avec `AVAudioEngine` et envoie le signal sur le réseau local après association à Patchbay. Prévoir un démarrage et un arrêt explicites sur l'iPad, un état de connexion côté Mac et la conservation du câblage lors d'une coupure. [Apple, AVAudioEngine](https://developer.apple.com/documentation/avfaudio/avaudioengine/inputnode)
  - **Essai filaire**. Si AUM est disponible sur l'iPad, tester le retour du micro intégré vers la sortie utilisée par IDAM. Le micro emprunte alors le même flux que le son lu sur l'iPad. [Guide AUM](https://www.kymatica.com/aum/help)
  - **Recette de la source dédiée**. Recevoir la voix et le son des apps sur deux blocs distincts. Vérifier les mutes indépendants, mesurer la latence et vérifier la reprise après interruption du réseau ou verrouillage de l'iPad.

Pour le son des apps sans fil, qualifier le récepteur AirPlay de macOS. Vérifier que Patchbay peut isoler le flux reçu du reste du son du Mac et mesurer sa latence avant de proposer une source dédiée. [Apple, récepteur AirPlay](https://support.apple.com/fr-fr/guide/mac-help/mchld7e543a0/mac)

## Effets audio et traitement intégrés

- [ ] **Traitement vocal natif**. Ajouter une chaîne d'effets directement sur une source microphone, avant son envoi vers les **Canaux de sortie** ou une destination. Inclure un égaliseur, un compresseur, un noise gate et un de-esser. Traiter le signal dans le moteur de Patchbay. Permettre de régler, réordonner et désactiver chaque effet, puis de retrouver la chaîne au rechargement du patch.
  - **Recette**. Envoyer un microphone traité vers un bus virtuel et une écoute physique. Vérifier l'effet de chaque réglage sur un signal connu, le bypass et la restauration de l'ordre et des paramètres.

Rogue Amoeba a annoncé le 15 janvier 2026 l'ajout d'effets audio à Loopback, notamment pour simplifier le traitement du microphone. Son guide décrit le parcours avec Audio Hijack qui a motivé cette annonce. [Annonce de Rogue Amoeba](https://weblog.rogueamoeba.com/2026/01/15/rogue-amoebas-2026-status-report/) · [Guide du traitement du microphone](https://rogueamoeba.com/support/knowledgebase/?showArticle=MicEffects)

## Ergonomie et contrôle avancé

- [ ] **MIDI et Stream Deck**. Proposer une affectation MIDI intégrée et des actions Stream Deck pour piloter un gain, le mute d'une source ou le basculement vers un bus virtuel ou une configuration cible. Enregistrer les affectations et rendre leur état visible dans Patchbay.
  - **Recette**. En cours de routage, déplacer un fader MIDI, couper un micro depuis le Stream Deck et sélectionner la cible voulue. Vérifier la prise en compte des commandes, le retour d'état et la reprise après reconnexion du contrôleur.
- [ ] **Compensation de latence**. Permettre d'ajouter un délai par source, affiché en millisecondes et sauvegardé avec le patch, pour aligner par exemple un micro USB avec le son d'une application.
  - **Recette**. Injecter le même transitoire sur deux trajets. Mesurer leur écart en loopback avant et après compensation, avec la fréquence et les périphériques de test.
- [ ] **Correction de dérive des agrégats**. Rendre visibles la source d'horloge et la correction de dérive par sous-périphérique. Prévoir un réglage dans Patchbay quand il est disponible pour l'agrégat sélectionné.
  - **Recette**. Mesurer l'écart entre deux périphériques après plusieurs minutes. Vérifier le résultat de la correction et l'état affiché si l'agrégat est reconnecté.
- [ ] **Dossiers de bus virtuels**. Permettre de grouper les bus par usage dans des dossiers nommés et repliables. Conserver leur organisation après rechargement.
- [ ] **Couleurs de câbles**. Permettre de choisir et de sauvegarder la couleur d'une liaison. Vérifier que les liaisons restent lisibles et sélectionnables dans un patch à 64 canaux, y compris dans la fenêtre minimale.
- [ ] **Verrouillage d'une configuration**. Ajouter un verrou explicite qui protège le câblage, les blocs et les affectations des périphériques contre une modification accidentelle. Garder les faders, les mutes et les vumètres accessibles en mode verrouillé.
  - **Recette**. Verrouiller un patch multicanal, tenter de déplacer un bloc ou de supprimer un câble, puis vérifier que le réglage des niveaux reste accessible. Vérifier le retour à l'édition après déverrouillage.

## Routage et mixage

- [ ] **Recette des interfaces virtuelles natives**. Sélectionner deux interfaces Patchbay aux capacités d'entrée et de sortie différentes dans des applications distinctes, sans faire intervenir BlackHole. Envoyer des signaux distincts et contrôler les canaux PCM, l'isolation et la persistance des affectations après redémarrage. Vérifier aussi la réaffectation des anciens agrégats.
- [ ] **Mix-minus automatisé**. Une fois la source de l'invité, les sources à partager et le bus de retour choisis, générer leur câblage en un clic pour Zoom, Riverside ou Discord. Construire le retour avec le micro local et les sources partagées, en excluant la piste de l'invité. Pour plusieurs invités, créer un retour propre à chacun quand leurs entrées sont distinctes. Si l'application fournit une seule piste distante, utiliser ce mix comme source à exclure.
  - **Recette**. Avec des signaux distincts, vérifier que le retour contient le micro et les sources prévues et que la contribution de l'invité est absente. Vérifier que l'écoute locale reçoit l'invité et que les liaisons créées passent la validation de boucles de Patchbay.
- [ ] **Snapshots, import et export de profils**. Sauvegarder des instantanés nommés et les restaurer pour passer entre les configurations podcast, streaming et MAO. Exporter et importer des profils versionnés qui contiennent le graphe, les réglages du mixer, les bus et les affectations associées. À l'import, indiquer les périphériques manquants et proposer leur réaffectation avant démarrage du routage.
  - **Recette**. Exporter un profil, le réimporter dans une session isolée et comparer les liaisons, les gains, les mutes, les paires stéréo et le master. Refaire l'essai avec un périphérique absent et vérifier la restauration d'un snapshot.
- [ ] **Icône dans la barre des menus de macOS**. Afficher l'état du routage, les niveaux du master et les commandes de mute des sources de la configuration active. Synchroniser les actions avec le mixer de la fenêtre principale.
  - **Recette**. Fermer la fenêtre principale, vérifier les vumètres dans le menu et couper une source. Rouvrir la fenêtre et vérifier que son état et les niveaux correspondent.

# Améliorations de Patchbay

Demandes à réaliser. Les spécifications du projet sont dans [AGENTS.md](AGENTS.md).

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

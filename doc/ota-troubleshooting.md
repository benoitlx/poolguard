# OTA — notes de mise au point

Récap des pièges rencontrés en mettant en place le firmware de base (MQTT en boucle + OTA), à l'époque un binaire de bring-up séparé (`src/bin/mqtt_test.rs`), depuis promu en `src/main.rs`. Tout ce qui est décrit ici est désormais résolu et en place ; ce document reste comme référence à relire avant de retoucher au flash/partitions/toolchain, pas comme liste de bugs ouverts. Pour l'usage courant de l'OTA (déclenchement, image, etc.), voir [development.md](development.md) et [mqtt.md](mqtt.md).

## Toolchain

- **`rust-toolchain.toml` est obligatoire** (`channel = "esp"`). Sans ça, `cargo` utilise le toolchain stable par défaut, qui ne connaît pas le target `xtensa-esp32-espidf` — malgré ce que dit l'erreur, ce target existe bien (`rustc +esp --print target-list` le liste), il faut juste être sur le bon toolchain.
- `xtensa-esp-elf` (dans `~/export-esp.sh`) est le nom du **toolchain GCC/linker**, pas un target Rust. Ne pas confondre avec `xtensa-esp32-espidf` (le vrai target Rust).
- `build-std-features = ["panic_immediate_abort"]` dans `.cargo/config.toml` casse la compilation avec les nightly récents (`panic_immediate_abort is now a real panic strategy!`). Le template upstream l'a retiré — on a fait pareil, sans remplacement (le remplacement `panic = "immediate-abort"` a un bug amont non résolu avec `build-std`).

## Dépendances

- `esp-idf-svc` 0.49.1 a un bug connu (`*const i8` vs `*const u8`, cf. [esp-idf-template#252](https://github.com/esp-rs/esp-idf-template/issues/252)) qui empêche de compiler `tls.rs`/`cstr.rs`. Fix : bump vers `esp-idf-svc` 0.52, `esp-idf-hal` 0.46, `esp-idf-sys` 0.37, `embedded-svc` 0.29, `embuild` 0.33.
- Ce bump casse l'API existante (voir plus bas) — normal, ~1.5 an d'écart entre 0.49 et 0.52.

## Ethernet (API 0.52)

- `EthDriver::new_rmii` prend **14 arguments explicites** (mac + les 6 pins RMII fixes en silicium GPIO25/26/27/19/22/21 + MDC/MDIO GPIO23/18 + clock config + reset + chipset + phy_addr + sysloop) — pas 6 comme dans l'ancien code. Voir `start_ethernet()` dans `main.rs`.
- Les pins ne sont plus `Clone` en 0.46 — il faut les déplacer par valeur, donc `Peripherals` doit être pris par valeur (pas `&Peripherals`) dans la fonction qui monte l'ethernet.
- `EspEth<'static>` est devenu `EspEth<'static, RmiiEth>` (générique supplémentaire).

## MQTT

- `event.payload()` retourne `EventPayload<'_, EspError>` (de `embedded_svc::mqtt::client`), **pas** `EspMqttEvent::Received` — ce dernier n'a jamais existé, même en 0.49. Corrigé dans `src/mqtt.rs`.
- **Ne jamais appeler de méthode sur `EspMqttClient` (subscribe/publish) depuis le thread qui lit `conn.next()`.** Le mécanisme interne (`private::zerocopy::Channel`) est un rendez-vous bloquant : la tâche MQTT d'ESP-IDF reste bloquée dans le callback tant qu'on n'a pas rappelé `next()`. Appeler `subscribe()` depuis là = deadlock silencieux (le heartbeat semble juste s'arrêter, aucune erreur, et le broker finit par déconnecter pour `exceeded timeout`).
  - Pattern correct : le thread qui lit `conn` ne fait que transmettre des signaux/données via un channel (`mpsc`) ; un **autre** thread, qui ne touche jamais `conn`, fait les appels `subscribe`/`publish` sur le client (protégé par `Arc<Mutex<_>>`).
- `EspMqttClient::new()` ne bloque pas jusqu'à la connexion — un `subscribe()` synchrone juste après plante avec `Client has not connected` côté ESP-IDF. Il faut attendre l'événement `EventPayload::Connected` avant de subscribe.
- Stack sizes des threads MQTT : prévoir large (8192 pour le thread heartbeat/subscribe et le thread lecteur d'événements). Un stack overflow silencieux peut ressembler à un deadlock (aucune erreur, tout s'arrête).

## Partitions & OTA

- La table de partitions par défaut d'ESP-IDF (`factory` seul) **ne supporte pas l'OTA** — `EspOta::initiate_update()` échoue avec `ESP_ERR_INVALID_ARG` (pas de partition `ota_0`/`ota_1` où écrire). Il faut `CONFIG_PARTITION_TABLE_TWO_OTA=y` dans `sdkconfig.defaults`.
- Cette table (`factory` + `ota_0` 1M + `ota_1` 1M + otadata) dépasse la taille flash par défaut (2MB en Kconfig). Il faut aligner `CONFIG_ESPTOOLPY_FLASHSIZE_8MB=y` sur la vraie taille de la puce (visible dans le boot log : `SPI Flash Size : 8MB`). Attention : cette ligne du boot log vient de l'auto-détection matérielle, elle ne prouve *pas* que la config Kconfig a été appliquée — ne pas s'y fier comme vérification.
- **`CONFIG_PARTITION_TABLE_CUSTOM_FILENAME` est résolu relativement au dossier de build interne d'`esp-idf-sys`** (`target/.../esp-idf-sys-<hash>/out/`), pas à la racine du projet — impossible d'y pointer un fichier versionné dans le repo sans chemin absolu (donc non portable). On garde `CONFIG_PARTITION_TABLE_TWO_OTA=y` (table intégrée à ESP-IDF, vérifiée correcte) pour le *build*, et un `partitions.csv` local (même contenu, versionné) uniquement pour le flag `--partition-table` d'`espflash` au *flash*.
- **`espflash flash <elf>` ne suffit pas** pour une table de partitions non standard — il faut passer explicitement `--partition-table=partitions.csv` dans le runner (`.cargo/config.toml`), sinon la table réellement écrite sur la puce peut rester périmée même après un rebuild réussi et un chip erase complet.
- Une image OTA (à servir en HTTP) n'est **pas** le fichier ELF produit par `cargo build` — il faut le convertir : `espflash save-image --chip esp32 <elf> ota.bin` (sans `--merge`, on ne veut que l'image app, pas bootloader+partition-table).
- Changer la table de partitions nécessite un reflash complet par UART — l'OTA ne peut pas faire cette transition (elle ne réécrit qu'une partition app, pas la table elle-même).

## Cache de build

- `cargo build` peut afficher `Finished ... in 0.1s` et reflasher un **vieux** binaire sans erreur si le build script d'`esp-idf-sys` n'est pas relancé (fingerprint cargo qui ne détecte pas le changement, surtout après un build qui a échoué). Signe qu'il faut se méfier : un temps de build anormalement court après un changement de `sdkconfig.defaults`.
- En cas de doute sur ce qui est réellement flashé : `cargo clean -p esp-idf-sys` (ou `cargo clean` complet si ça persiste) avant de rebuild.
- Pour vérifier le contenu réel d'une table de partitions générée (sans avoir à flasher) :
  ```bash
  python3 .embuild/espressif/esp-idf/v5.2.2/components/partition_table/gen_esp32part.py <partition-table.bin>
  ```

## Matériel / UART

- WT32-ETH01 : logique 3.3V, **pas tolérant au 5V**. Vérifier le chip de l'adaptateur UART avant de brancher (CP2104 = 3.3V natif, safe ; CH340G = souvent 5V, vérifier un jumper).
- Câblage croisé : TX adaptateur → RX (GPIO3), RX adaptateur ← TX (GPIO1), GND commun, 3.3V → 3.3V.
- Mode download avant flash : IO0 LOW → pulse EN → relâcher IO0. Après flash : EN pour redémarrer normalement (IO0 doit être relâché, pas au LOW, pour démarrer le firmware au lieu du bootloader ROM).
- `Permission denied` sur `/dev/ttyUSB0` : ajouter l'utilisateur au groupe `dialout` (`sudo usermod -aG dialout $USER`) + déconnexion/reconnexion de session complète.
- La carte WT32-ETH01 consomme plus qu'un adaptateur UART seul (PHY Ethernet actif) — un adaptateur qui ne sort que ~50mA en 3.3V peut brown-out la carte *et* se déconnecter lui-même en USB sous charge.

## Procédure de test OTA complète

Voir aussi le `justfile` (`just ota-image`, `just ota-serve`, `just ota-push`) qui enchaîne ces mêmes étapes.

```bash
# 1. Build
MQTT_USERNAME=<user> MQTT_PASSWORD=<pass> cargo build --release

# 2. Générer l'image flashable (app seule)
espflash save-image --chip esp32 target/xtensa-esp32-espidf/release/poolguard ota.bin

# 3. Servir en HTTP
python3 -m http.server 8080 --directory .

# 4. Déclencher (URL sans guillemets dans le payload !)
mosquitto_pub -h 192.168.1.142 -u <user> -P <pass> \
  -t pool/cmd/ota -m http://<ip-machine-dev>:8080/ota.bin
```

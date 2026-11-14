# YR

YT Music Rust — Cliente de música **100% en Rust** para Android (y escritorio), con interfaz oscura estilo YT Music, reproducción en streaming/offline y gestor de descargas concurrente.

## Arquitectura (Cargo Workspace)

- [`yr-core`](crates/yr-core): Modelos de dominio (`Track`, `AlbumSummary`, `PlaylistSummary`, `AudioQuality`), cola de reproducción (`PlaybackQueue` con shuffle y modos de repetición) y almacenamiento persistente atómico (`LibraryStore`: favoritos, historial y playlists locales).
- [`yr-provider`](crates/yr-provider): Trait `MusicProvider` e implementación `YtMusicProvider` sobre `rustypipe` (Rust puro + `rustls`), con búsqueda de canciones/álbumes/playlists, tendencias y resolución de streams de audio.
- [`yr-download`](crates/yr-download): Gestor de descargas concurrente (`DownloadManager`) con cola por prioridad, reanudación HTTP `Range` sobre archivos `.part`, reintentos con backoff exponencial, cálculo de velocidad/ETA e índice offline con metadatos.
- [`yr-audio`](crates/yr-audio): Motor de audio en hilo dedicado (`AudioEngine`) usando `rodio` + `symphonia` (AAC/M4A, MP3, FLAC, Vorbis, WAV) con reproducción local instantánea y lector HTTP progresivo (`Read + Seek`).
- [`yr-app`](crates/yr-app): Interfaz gráfica nativa en Rust puro con `egui` / `eframe` (`android-native-activity` para Android + binario de escritorio).

## Compilación

### GitHub Actions (APK Android)

El workflow [`.github/workflows/ci.yml`](.github/workflows/ci.yml) ejecuta automáticamente:
1. Pruebas unitarias (`cargo test -p yr-core -p yr-provider -p yr-download`).
2. Compilación del APK release firmado para `aarch64-linux-android` y `armv7-linux-androideabi` usando `cargo-apk`, publicado como artefacto `yr-music-android-apk`.

### Escritorio (Local)

```bash
cargo run -p yr-app --bin yr
```

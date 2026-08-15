# Melodie companion (Android)

A phone client for Melodie's LAN server (`docs/lan.md`). The desktop is the
librarian; the phone is a mirror — it never downloads from YouTube, never
matches tracks, and never contacts Spotify.

## Installing

The build is at `melodie-companion.apk` in the repo root.

1. Get the file onto the phone — USB, an email to yourself, a cloud drive,
   whatever is easiest.
2. Tap it in the phone's file manager or notification shade.
3. Android asks whether to allow installs from that app (Files, Chrome,
   Drive…). Allow it. This is per-app and can be turned back off after.
4. Tap Install.

No Play Store, no account. The APK is signed with the standard Android debug
key, which is fine for sideloading and means there is no keystore to keep
track of.

## Pairing

1. On the desktop, enable the LAN server (`docs/lan.md`) and restart Melodie.
2. Click **Pair** in the Melodie window — it shows a QR code.
3. Open Melodie on the phone and tap **Scan QR code**.

Phone and computer must be on the same Wi-Fi. If the camera is awkward,
`melodie --pair` prints the host, port and token to type in by hand.

## Using it

- The first screen lists the playlists on the desktop.
- Tap one to see its tracks, then **Sync … to this phone** to download them
  into app-private storage.
- Search and Sort (Title/Artist/Album/Duration) live at the top of a
  playlist's track list, filtering/reordering it live — the same behavior
  as the desktop window. Tap-to-play always queues from what's currently
  on screen, so a search or sort in effect changes what Next/Previous walk
  through, not just what's listed.
- Tap a synced track to play. Playback continues with the screen off and the
  app closed, with lockscreen and Bluetooth controls.
- The Player screen has a Shuffle button next to Previous/Next; it stays on
  across playlists until toggled off.
- A "‹" back button sits at the top of every screen except the playlist
  list (the app's home) — you don't have to rely on the phone's gesture/
  system back to leave the Player screen.
- Synced tracks play with no network at all — airplane mode included.

Re-syncing a playlist downloads only what's missing. Tracks removed from a
playlist upstream stop being listed but their files are not deleted, matching
the desktop's rule that a remote change never destroys local audio.

## Build

Requires JDK 17+ and the Android SDK (platform 36, build-tools 36.0.0):

```sh
cd android
./gradlew assembleRelease     # shrunk, debug-signed
./gradlew assembleDebug       # unshrunk, faster
./gradlew :app:testDebugUnitTest
```

`android/local.properties` must point `sdk.dir` at the SDK.

## Deliberate limits

- Sync runs only while the app is open and you tap Sync — no background
  scheduler (no WorkManager).
- Playlists sync individually; there is no "mirror everything" button.
- No cover art on the phone yet: the server serves `getCoverArt`, but the
  app does not request it, which keeps the sync to audio bytes only.

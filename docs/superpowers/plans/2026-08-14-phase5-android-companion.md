# Phase 5 — Android Companion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A sideloadable Android app that pairs with the desktop by QR code, syncs chosen playlists to phone-local storage over Melodie's Subsonic API, and plays them offline in the background with lockscreen controls.

**Architecture:** One Gradle module. Kotlin, Views built in code (no Compose, no XML layouts, no AppCompat). Networking is `HttpURLConnection`; XML is parsed with a streaming `XmlPullParser`; the local manifest is a tab-separated file. Playback is Media3 `ExoPlayer` inside a `MediaSessionService`, which is the only supported way to keep audio alive when the app is not visible. The phone never matches, downloads from YouTube, or contacts Spotify — the desktop is the librarian.

**Tech Stack:** Kotlin 2.2.21, AGP 8.13.2, Gradle 8.14.3, JDK 21 (already installed), compileSdk/targetSdk 36, minSdk 24, `androidx.media3` 1.11.0 (`exoplayer` + `session`), `com.journeyapps:zxing-android-embedded` 4.3.0.

**Spec:** `docs/superpowers/specs/2026-08-14-lan-subsonic-and-android-design.md`

**Depends on:** `docs/superpowers/plans/2026-08-14-phase4-lan-subsonic.md` must be complete — this app is a client of that server. Endpoints used: `ping`, `getPlaylists`, `getPlaylist`, `download`, `getCoverArt`.

## Global Constraints

- **PLAN.md §7 is binding:** Kotlin, Views (no Compose), `minSdk 24`, Media3 `ExoPlayer` inside a `MediaSessionService`, offline-first (plays from app-private storage, works in airplane mode), QR pairing carrying host/port/token, no login.
- **The phone runs no librarian logic.** No yt-dlp, no matching, no Spotify contact, and no writes back to the desktop library. Every server call is a GET against a read-only endpoint.
- **Dependency budget: three libraries, total.** `media3-exoplayer`, `media3-session`, `zxing-android-embedded`. Everything else comes from the Android/Kotlin standard library. Specifically *not* used: AppCompat, Compose, Room, OkHttp/Retrofit, WorkManager, Coroutines, Glide/Coil, Dagger/Hilt. Adding any of these needs a stated reason that a few lines of stdlib cannot cover.
- **No XML layouts.** Views are constructed in Kotlin. This keeps the resource table and `aapt` output near-empty and avoids AppCompat entirely.
- **Memory discipline mirrors PLAN.md §6.** Parse XML by streaming (never DOM — a large playlist response would otherwise materialise entirely in RAM), download to disk with a fixed buffer (never `readBytes()` into memory), one download at a time (mirrors the desktop's serialized fetch policy in PLAN.md §5.5). No polling timers except the player screen's position tick, which runs only while that screen is foregrounded.
- **Package name:** `dev.melodie.companion`. **Root project name:** `melodie-companion`.
- **SDK location:** `~/Android/Sdk`, written to `android/local.properties`. Additive — do not modify anything outside `android/` and that SDK directory.
- **Signing:** the release build type is signed with Gradle's **debug** keystore, deliberately. The user chose no keystore management; the release build type is used only for its R8 shrinking.
- **This repo is not a git repository.** Where the standard plan format says "commit", run the verification command instead.
- **No emulator or device is available on this machine.** Verification is: unit tests pass, the build succeeds, and the APK is structurally checked with `aapt2`/`apkanalyzer`. On-device behaviour is verified by the user after sideloading. Never claim on-device behaviour was tested.

---

### Task 1: Toolchain bootstrap and a buildable empty app

Ends with a real `.apk` on disk. Everything after this adds behaviour to something that already builds.

**Files:**
- Create: `android/settings.gradle.kts`, `android/build.gradle.kts`, `android/gradle.properties`, `android/local.properties`, `android/.gitignore`
- Create: `android/app/build.gradle.kts`, `android/app/proguard-rules.pro`
- Create: `android/app/src/main/AndroidManifest.xml`
- Create: `android/app/src/main/res/values/strings.xml`, `android/app/src/main/res/drawable/ic_launcher_foreground.xml`, `android/app/src/main/res/values/ic_launcher_background.xml`, `android/app/src/main/res/mipmap-anydpi-v26/ic_launcher.xml`
- Create: `android/app/src/main/java/dev/melodie/companion/PairActivity.kt` (placeholder, replaced in Task 4)

**Interfaces:**
- Produces: a Gradle wrapper at `android/gradlew`, and `android/app/build/outputs/apk/debug/app-debug.apk`.

- [ ] **Step 1: Install the Android command-line tools**

```sh
mkdir -p ~/Android/Sdk/cmdline-tools
cd /tmp/claude-1000
curl -L -o cmdline-tools.zip \
  https://dl.google.com/android/repository/commandlinetools-linux-15859902_latest.zip
unzip -q cmdline-tools.zip -d ~/Android/Sdk/cmdline-tools
mv ~/Android/Sdk/cmdline-tools/cmdline-tools ~/Android/Sdk/cmdline-tools/latest
~/Android/Sdk/cmdline-tools/latest/bin/sdkmanager --version
```

Expected: prints a version number (e.g. `13.0`).

- [ ] **Step 2: Accept licences and install the SDK packages**

```sh
export ANDROID_HOME=$HOME/Android/Sdk
yes | $ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager --licenses > /dev/null
$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager \
  "platform-tools" "platforms;android-36" "build-tools;36.0.0"
ls $ANDROID_HOME/platforms $ANDROID_HOME/build-tools
```

Expected: `android-36` and `36.0.0` are listed. (compileSdk 36 is not a preference — `media3-common:1.11.0`'s AAR metadata declares `minCompileSdk=36`, so the build fails without it.)

- [ ] **Step 3: Install Gradle 8.14.3 and generate the wrapper**

No Gradle is installed, and the wrapper cannot bootstrap itself. Install one copy, use it once to generate the wrapper, and use `./gradlew` from then on.

```sh
cd /tmp/claude-1000
curl -L -o gradle.zip https://services.gradle.org/distributions/gradle-8.14.3-bin.zip
unzip -q gradle.zip -d "$HOME/.local/share"
mkdir -p /home/yuzy/Melodie/android
cd /home/yuzy/Melodie/android
"$HOME/.local/share/gradle-8.14.3/bin/gradle" wrapper --gradle-version 8.14.3
./gradlew --version
```

Expected: `./gradlew --version` prints Gradle 8.14.3 and JVM 21.

- [ ] **Step 4: Write the Gradle configuration**

`android/settings.gradle.kts`:

```kotlin
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "melodie-companion"
include(":app")
```

`android/build.gradle.kts`:

```kotlin
plugins {
    id("com.android.application") version "8.13.2" apply false
    id("org.jetbrains.kotlin.android") version "2.2.21" apply false
}
```

`android/gradle.properties`:

```properties
org.gradle.jvmargs=-Xmx2048m -Dfile.encoding=UTF-8
org.gradle.caching=true
android.useAndroidX=true
android.nonTransitiveRClass=true
kotlin.code.style=official
```

`android/local.properties` (machine-specific; absolute path required by AGP):

```properties
sdk.dir=/home/yuzy/Android/Sdk
```

`android/.gitignore`:

```
local.properties
.gradle/
build/
app/build/
```

- [ ] **Step 5: Write the app module's build file**

`android/app/build.gradle.kts`:

```kotlin
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.melodie.companion"
    // Not a preference: media3 1.11.0's AAR metadata declares minCompileSdk=36.
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.melodie.companion"
        minSdk = 24          // PLAN.md §7
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            // R8 is the whole reason this build type exists: Media3 alone is
            // several MB unshrunk.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // Debug-signed on purpose: this is a personal sideload, so there
            // is no keystore for the user to keep track of.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // Nothing here needs BuildConfig, view binding, Compose, or resource
    // values — leaving them off keeps the build and the APK smaller.
    buildFeatures {
        buildConfig = false
    }

    packaging {
        resources.excludes += setOf(
            "META-INF/*.version",
            "META-INF/*.kotlin_module",
            "DebugProbesKt.bin",
        )
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    // PLAN.md §1/§7: the researched, non-negotiable way to get background
    // playback, audio focus, and lockscreen controls on Android.
    implementation("androidx.media3:media3-exoplayer:1.11.0")
    implementation("androidx.media3:media3-session:1.11.0")
    // QR pairing. Chosen over ML Kit because ML Kit requires Google Play
    // Services; this works on stock AOSP and depends only on zxing-core.
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")

    testImplementation("junit:junit:4.13.2")
    // Test-only XmlPullParser implementation, so the response parser can be
    // tested on the JVM. Never packaged into the APK.
    testImplementation("net.sf.kxml:kxml2:2.3.0")
}
```

`android/app/proguard-rules.pro`:

```proguard
# Media3, ZXing and AndroidX ship their own consumer rules, and AGP keeps
# anything named in the manifest. Nothing here uses reflection, so this file
# stays empty on purpose.
```

- [ ] **Step 6: Write the manifest and resources**

`android/app/src/main/AndroidManifest.xml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">

    <uses-permission android:name="android.permission.INTERNET" />
    <uses-permission android:name="android.permission.CAMERA" />
    <uses-permission android:name="android.permission.POST_NOTIFICATIONS" />
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE" />
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK" />

    <application
        android:allowBackup="false"
        android:icon="@mipmap/ic_launcher"
        android:label="@string/app_name"
        android:theme="@android:style/Theme.Material.NoActionBar"
        android:usesCleartextTraffic="true">

        <activity
            android:name=".PairActivity"
            android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>

    </application>
</manifest>
```

`android:usesCleartextTraffic="true"` is required: the Subsonic server is plain HTTP on a LAN address, and Android blocks cleartext by default from API 28.

`android/app/src/main/res/values/strings.xml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <string name="app_name">Melodie</string>
</resources>
```

`android/app/src/main/res/values/ic_launcher_background.xml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">#1B1B1F</color>
</resources>
```

`android/app/src/main/res/drawable/ic_launcher_foreground.xml` — a vector so no bitmaps ship:

```xml
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="108dp"
    android:height="108dp"
    android:viewportWidth="108"
    android:viewportHeight="108">
    <path
        android:fillColor="#E8B44A"
        android:pathData="M62,28 L62,62 A10,10 0 1,1 56,53 L56,40 L44,43 L44,68 A10,10 0 1,1 38,59 L38,38 Z" />
</vector>
```

`android/app/src/main/res/mipmap-anydpi-v26/ic_launcher.xml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background" />
    <foreground android:drawable="@drawable/ic_launcher_foreground" />
</adaptive-icon>
```

Pre-API-26 devices need a non-adaptive fallback at the same resource name. `android/app/src/main/res/drawable/ic_launcher.xml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<layer-list xmlns:android="http://schemas.android.com/apk/res/android">
    <item android:drawable="@color/ic_launcher_background" />
    <item android:drawable="@drawable/ic_launcher_foreground" />
</layer-list>
```

and reference it from a `mipmap` alias — `android/app/src/main/res/mipmap-anydpi/ic_launcher.xml` is not valid, so instead change the manifest's icon attribute to `android:icon="@drawable/ic_launcher"` and delete the `mipmap-anydpi-v26` file *only if* the adaptive icon fails to build. Try the adaptive icon first; on API 24-25 Android falls back to the same drawable automatically because `@mipmap/ic_launcher` resolves through the v26 qualifier only on 26+. If `aapt2` errors with "resource mipmap/ic_launcher not found", add `android/app/src/main/res/mipmap/ic_launcher.xml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<layer-list xmlns:android="http://schemas.android.com/apk/res/android">
    <item android:drawable="@color/ic_launcher_background" />
    <item android:drawable="@drawable/ic_launcher_foreground" />
</layer-list>
```

- [ ] **Step 7: Write a placeholder launcher activity**

`android/app/src/main/java/dev/melodie/companion/PairActivity.kt` (replaced wholesale in Task 4):

```kotlin
package dev.melodie.companion

import android.app.Activity
import android.os.Bundle
import android.widget.TextView

class PairActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = "Melodie" })
    }
}
```

- [ ] **Step 8: Build the debug APK**

```sh
cd /home/yuzy/Melodie/android
./gradlew assembleDebug
```

Expected: `BUILD SUCCESSFUL`.

- [ ] **Step 9: Verify the APK exists and is well-formed**

```sh
ls -lh app/build/outputs/apk/debug/app-debug.apk
$HOME/Android/Sdk/build-tools/36.0.0/aapt2 dump badging \
  app/build/outputs/apk/debug/app-debug.apk | head -5
```

Expected: the file exists, and badging reports `package: name='dev.melodie.companion'` with `sdkVersion:'24'` and `targetSdkVersion:'36'`.

---

### Task 2: Subsonic client — auth, requests, streaming XML parse

The one piece with real logic, and the one that must be testable without a device.

**Files:**
- Create: `android/app/src/main/java/dev/melodie/companion/Pairing.kt`
- Create: `android/app/src/main/java/dev/melodie/companion/Subsonic.kt`
- Create: `android/app/src/test/java/dev/melodie/companion/SubsonicTest.kt`

**Interfaces:**
- Produces:
  - `data class Server(val host: String, val port: Int, val token: String)`
  - `fun parsePairingUrl(raw: String): Server?`
  - `data class RemotePlaylist(val id: String, val name: String, val songCount: Int)`
  - `data class RemoteSong(val id: String, val title: String, val artist: String, val album: String, val durationSec: Int, val suffix: String)`
  - `class SubsonicException(val code: Int, message: String) : Exception(message)`
  - `object Subsonic { fun md5(s: String): String; fun authQuery(token: String, salt: String): String; fun url(server: Server, method: String, params: String): String; fun <T> parseAll(parser: XmlPullParser, tag: String, map: (XmlPullParser) -> T): List<T> }`
  - `class SubsonicClient(val server: Server) { fun ping(): Boolean; fun playlists(): List<RemotePlaylist>; fun playlist(id: String): List<RemoteSong>; fun downloadTo(songId: String, target: File, onBytes: (Long) -> Unit) }`

- [ ] **Step 1: Write the failing tests**

`android/app/src/test/java/dev/melodie/companion/SubsonicTest.kt`:

```kotlin
package dev.melodie.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.kxml2.io.KXmlParser
import org.xmlpull.v1.XmlPullParser
import java.io.StringReader

class SubsonicTest {

    private fun parserFor(xml: String): XmlPullParser =
        KXmlParser().apply {
            setFeature(XmlPullParser.FEATURE_PROCESS_NAMESPACES, false)
            setInput(StringReader(xml))
        }

    @Test
    fun md5MatchesTheServersSaltedTokenScheme() {
        // Known vector: md5("abc") — the desktop computes md5(token + salt)
        // with exactly this function, so a mismatch here means no request
        // ever authenticates.
        assertEquals("900150983cd24fb0d6963f7d28e17f72", Subsonic.md5("abc"))
        assertEquals("d41d8cd98f00b204e9800998ecf8427e", Subsonic.md5(""))
    }

    @Test
    fun authQueryCarriesTokenAndSaltNotThePassword() {
        val q = Subsonic.authQuery("s3cret", "abc")
        assertTrue(q, q.contains("s=abc"))
        assertTrue(q, q.contains("t=" + Subsonic.md5("s3cretabc")))
        assertTrue(q, q.contains("v=1.16.1"))
        assertTrue(q, q.contains("c=melodie"))
        assertTrue("the shared secret must never travel in clear", !q.contains("s3cret&") && !q.contains("p="))
    }

    @Test
    fun urlBuildsAgainstTheServerAddress() {
        val server = Server("192.168.1.20", 4533, "tok")
        val url = Subsonic.url(server, "getPlaylists", "")
        assertTrue(url, url.startsWith("http://192.168.1.20:4533/rest/getPlaylists.view?"))
    }

    @Test
    fun parsePairingUrlAcceptsTheDesktopsFormat() {
        val s = parsePairingUrl("melodie://192.168.1.20:4533/abcTOKEN123")
        assertEquals("192.168.1.20", s!!.host)
        assertEquals(4533, s.port)
        assertEquals("abcTOKEN123", s.token)

        assertNull(parsePairingUrl("http://192.168.1.20:4533/tok"))
        assertNull(parsePairingUrl("melodie://192.168.1.20/tok"))
        assertNull(parsePairingUrl("melodie://192.168.1.20:notaport/tok"))
        assertNull(parsePairingUrl("nonsense"))
        assertNull(parsePairingUrl("melodie://192.168.1.20:4533/"))
    }

    @Test
    fun parsesPlaylistListing() {
        val xml = """
            <?xml version="1.0" encoding="UTF-8"?>
            <subsonic-response status="ok" version="1.16.1">
              <playlists>
                <playlist id="pl1" name="Chill" songCount="2" duration="420"/>
                <playlist id="pl2" name="Rock &amp; Roll" songCount="17" duration="3600"/>
              </playlists>
            </subsonic-response>
        """.trimIndent()
        val out = Subsonic.parseAll(parserFor(xml), "playlist") {
            RemotePlaylist(
                it.getAttributeValue(null, "id"),
                it.getAttributeValue(null, "name"),
                it.getAttributeValue(null, "songCount")?.toIntOrNull() ?: 0,
            )
        }
        assertEquals(2, out.size)
        assertEquals("Chill", out[0].name)
        assertEquals(2, out[0].songCount)
        assertEquals("Rock & Roll", out[1].name, )
    }

    @Test
    fun parsesPlaylistEntries() {
        val xml = """
            <subsonic-response status="ok" version="1.16.1">
              <playlist id="pl1" name="Chill" songCount="1">
                <entry id="tr7" title="Rhubarb" artist="Aphex Twin" album="SAW II"
                       duration="210" suffix="mp3" contentType="audio/mpeg"/>
              </playlist>
            </subsonic-response>
        """.trimIndent()
        val out = Subsonic.parseAll(parserFor(xml), "entry") {
            RemoteSong(
                it.getAttributeValue(null, "id"),
                it.getAttributeValue(null, "title"),
                it.getAttributeValue(null, "artist") ?: "",
                it.getAttributeValue(null, "album") ?: "",
                it.getAttributeValue(null, "duration")?.toIntOrNull() ?: 0,
                it.getAttributeValue(null, "suffix") ?: "mp3",
            )
        }
        assertEquals(1, out.size)
        assertEquals("tr7", out[0].id)
        assertEquals(210, out[0].durationSec)
        assertEquals("mp3", out[0].suffix)
    }

    @Test
    fun serverErrorBecomesAnException() {
        val xml = """
            <subsonic-response status="failed" version="1.16.1">
              <error code="40" message="Wrong username or password"/>
            </subsonic-response>
        """.trimIndent()
        try {
            Subsonic.parseAll(parserFor(xml), "playlist") { }
            throw AssertionError("expected a SubsonicException")
        } catch (e: SubsonicException) {
            assertEquals(40, e.code)
            assertTrue(e.message!!, e.message!!.contains("Wrong username"))
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cd /home/yuzy/Melodie/android && ./gradlew :app:testDebugUnitTest
```

Expected: FAIL — `Unresolved reference: Subsonic`.

- [ ] **Step 3: Implement `Pairing.kt`**

```kotlin
package dev.melodie.companion

import android.content.Context

/** Where the desktop lives, and the secret that gets in. No accounts. */
data class Server(val host: String, val port: Int, val token: String)

/**
 * Parses the payload the desktop's QR code carries:
 * `melodie://<host>:<port>/<token>`.
 *
 * Hand-parsed rather than via `Uri.parse` so it can be unit-tested off-device
 * — `android.net.Uri` is a stub in JVM tests.
 */
fun parsePairingUrl(raw: String): Server? {
    val rest = raw.trim().removePrefix("melodie://")
    if (rest == raw.trim()) return null
    val slash = rest.indexOf('/')
    if (slash <= 0) return null
    val authority = rest.substring(0, slash)
    val token = rest.substring(slash + 1).trim()
    if (token.isEmpty()) return null
    val colon = authority.lastIndexOf(':')
    if (colon <= 0) return null
    val port = authority.substring(colon + 1).toIntOrNull() ?: return null
    if (port !in 1..65535) return null
    val host = authority.substring(0, colon)
    if (host.isEmpty()) return null
    return Server(host, port, token)
}

/** The paired server, remembered across launches. */
object Pairing {
    private const val PREFS = "pairing"

    fun load(context: Context): Server? {
        val p = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val host = p.getString("host", null) ?: return null
        val token = p.getString("token", null) ?: return null
        return Server(host, p.getInt("port", 4533), token)
    }

    fun save(context: Context, server: Server) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString("host", server.host)
            .putInt("port", server.port)
            .putString("token", server.token)
            .apply()
    }

    fun clear(context: Context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().clear().apply()
    }
}
```

- [ ] **Step 4: Implement `Subsonic.kt`**

```kotlin
package dev.melodie.companion

import android.util.Xml
import org.xmlpull.v1.XmlPullParser
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.security.MessageDigest

data class RemotePlaylist(val id: String, val name: String, val songCount: Int)

data class RemoteSong(
    val id: String,
    val title: String,
    val artist: String,
    val album: String,
    val durationSec: Int,
    val suffix: String,
)

class SubsonicException(val code: Int, message: String) : Exception(message)

object Subsonic {
    const val API_VERSION = "1.16.1"
    private const val CLIENT = "melodie"

    fun md5(input: String): String {
        val digest = MessageDigest.getInstance("MD5").digest(input.toByteArray(Charsets.UTF_8))
        val out = StringBuilder(digest.size * 2)
        for (b in digest) {
            val v = b.toInt() and 0xff
            out.append("0123456789abcdef"[v ushr 4])
            out.append("0123456789abcdef"[v and 0x0f])
        }
        return out.toString()
    }

    /**
     * Subsonic's salted-token auth: the shared secret itself never travels,
     * only md5(secret + salt) with a fresh salt per request.
     */
    fun authQuery(token: String, salt: String): String =
        "u=melodie&t=${md5(token + salt)}&s=$salt&v=$API_VERSION&c=$CLIENT"

    private fun freshSalt(): String {
        val alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
        val rnd = java.security.SecureRandom()
        return (1..12).map { alphabet[rnd.nextInt(alphabet.length)] }.joinToString("")
    }

    fun url(server: Server, method: String, params: String): String {
        val auth = authQuery(server.token, freshSalt())
        val extra = if (params.isEmpty()) "" else "&$params"
        return "http://${server.host}:${server.port}/rest/$method.view?$auth$extra"
    }

    fun encode(value: String): String = URLEncoder.encode(value, "UTF-8")

    /**
     * Streams through the response pulling out every `<tag>` element.
     *
     * Deliberately a pull parser, not DOM: a playlist response for a large
     * library would otherwise be materialised whole in RAM on the phone.
     */
    fun <T> parseAll(parser: XmlPullParser, tag: String, map: (XmlPullParser) -> T): List<T> {
        val out = ArrayList<T>()
        var event = parser.eventType
        while (event != XmlPullParser.END_DOCUMENT) {
            if (event == XmlPullParser.START_TAG) {
                when (parser.name) {
                    "error" -> throw SubsonicException(
                        parser.getAttributeValue(null, "code")?.toIntOrNull() ?: 0,
                        parser.getAttributeValue(null, "message") ?: "Unknown server error",
                    )
                    tag -> out.add(map(parser))
                }
            }
            event = parser.next()
        }
        return out
    }

    fun newParser(input: InputStream): XmlPullParser = Xml.newPullParser().apply {
        setFeature(XmlPullParser.FEATURE_PROCESS_NAMESPACES, false)
        setInput(input, null)
    }
}

/**
 * Every call is a blocking GET — callers run it on a background thread.
 * There is no connection pooling, no retry policy and no cache: this talks
 * to one server on the same Wi-Fi, a handful of times per sync.
 */
class SubsonicClient(val server: Server) {

    private fun open(method: String, params: String = ""): HttpURLConnection {
        val connection = URL(Subsonic.url(server, method, params)).openConnection() as HttpURLConnection
        connection.connectTimeout = 5_000
        connection.readTimeout = 20_000
        connection.requestMethod = "GET"
        return connection
    }

    private fun <T> get(method: String, params: String, tag: String, map: (XmlPullParser) -> T): List<T> {
        val connection = open(method, params)
        try {
            connection.inputStream.use { return Subsonic.parseAll(Subsonic.newParser(it), tag, map) }
        } finally {
            connection.disconnect()
        }
    }

    fun ping(): Boolean = try {
        get("ping", "", "nothing") { }
        true
    } catch (e: Exception) {
        false
    }

    fun playlists(): List<RemotePlaylist> = get("getPlaylists", "", "playlist") {
        RemotePlaylist(
            it.getAttributeValue(null, "id") ?: "",
            it.getAttributeValue(null, "name") ?: "(unnamed)",
            it.getAttributeValue(null, "songCount")?.toIntOrNull() ?: 0,
        )
    }

    fun playlist(id: String): List<RemoteSong> =
        get("getPlaylist", "id=${Subsonic.encode(id)}", "entry") { toSong(it) }

    private fun toSong(p: XmlPullParser) = RemoteSong(
        p.getAttributeValue(null, "id") ?: "",
        p.getAttributeValue(null, "title") ?: "(untitled)",
        p.getAttributeValue(null, "artist") ?: "",
        p.getAttributeValue(null, "album") ?: "",
        p.getAttributeValue(null, "duration")?.toIntOrNull() ?: 0,
        p.getAttributeValue(null, "suffix") ?: "mp3",
    )

    /**
     * Downloads to a temp file and renames on success, so an interrupted
     * sync can never leave a half-file that looks synced.
     *
     * Copies through a fixed 64 KB buffer — the file never exists in memory.
     */
    fun downloadTo(songId: String, target: File, onBytes: (Long) -> Unit) {
        val connection = open("download", "id=${Subsonic.encode(songId)}")
        val partial = File(target.parentFile, target.name + ".part")
        try {
            if (connection.responseCode != 200) {
                throw SubsonicException(connection.responseCode, "download failed for $songId")
            }
            connection.inputStream.use { input ->
                FileOutputStream(partial).use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        output.write(buffer, 0, read)
                        total += read
                        onBytes(total)
                    }
                }
            }
            if (!partial.renameTo(target)) throw SubsonicException(0, "could not finalise ${target.name}")
        } catch (e: Exception) {
            partial.delete()
            throw e
        } finally {
            connection.disconnect()
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```sh
cd /home/yuzy/Melodie/android && ./gradlew :app:testDebugUnitTest
```

Expected: PASS, 7 tests.

- [ ] **Step 6: Verify the app still builds**

```sh
./gradlew assembleDebug
```

Expected: `BUILD SUCCESSFUL`.

---

### Task 3: Local library manifest

What the phone knows about what it has on disk. A tab-separated file, so it can be read and written with no JSON dependency and tested on the JVM.

**Files:**
- Create: `android/app/src/main/java/dev/melodie/companion/LocalLibrary.kt`
- Create: `android/app/src/test/java/dev/melodie/companion/LocalLibraryTest.kt`

**Interfaces:**
- Consumes: `RemoteSong` (Task 2).
- Produces:
  - `data class LocalSong(val id: String, val title: String, val artist: String, val album: String, val durationSec: Int, val fileName: String, val playlist: String)`
  - `object LocalLibrary { fun encodeLine(s: LocalSong): String; fun decodeLine(line: String): LocalSong?; fun load(dir: File): List<LocalSong>; fun save(dir: File, songs: List<LocalSong>); fun tracksDir(dir: File): File; fun fileFor(dir: File, song: LocalSong): File }`

- [ ] **Step 1: Write the failing tests**

`android/app/src/test/java/dev/melodie/companion/LocalLibraryTest.kt`:

```kotlin
package dev.melodie.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class LocalLibraryTest {

    @get:Rule
    val temp = TemporaryFolder()

    private fun song(id: String, title: String, playlist: String = "Chill") = LocalSong(
        id = id,
        title = title,
        artist = "Aphex Twin",
        album = "SAW II",
        durationSec = 210,
        fileName = "$id.mp3",
        playlist = playlist,
    )

    @Test
    fun linesRoundTrip() {
        val s = song("tr7", "Rhubarb")
        assertEquals(s, LocalLibrary.decodeLine(LocalLibrary.encodeLine(s)))
    }

    @Test
    fun tabsAndNewlinesInMetadataCannotCorruptTheFile() {
        val s = song("tr8", "Weird\tTitle\nSecond line")
        val decoded = LocalLibrary.decodeLine(LocalLibrary.encodeLine(s))!!
        assertEquals("tr8", decoded.id)
        assertEquals(1, LocalLibrary.encodeLine(s).lines().size)
        assertEquals("Weird Title Second line", decoded.title)
    }

    @Test
    fun malformedLinesAreSkippedNotFatal() {
        assertNull(LocalLibrary.decodeLine(""))
        assertNull(LocalLibrary.decodeLine("only\ttwo"))
        assertNull(LocalLibrary.decodeLine("#comment"))
    }

    @Test
    fun saveThenLoadPreservesEverything() {
        val dir = temp.newFolder()
        val songs = listOf(song("tr1", "Rhubarb"), song("tr2", "Curtains", playlist = "Focus"))
        LocalLibrary.save(dir, songs)
        assertEquals(songs, LocalLibrary.load(dir))
    }

    @Test
    fun loadOnAFreshInstallIsEmptyNotAnError() {
        assertEquals(emptyList<LocalSong>(), LocalLibrary.load(temp.newFolder()))
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```sh
cd /home/yuzy/Melodie/android && ./gradlew :app:testDebugUnitTest
```

Expected: FAIL — `Unresolved reference: LocalLibrary`.

- [ ] **Step 3: Implement `LocalLibrary.kt`**

```kotlin
package dev.melodie.companion

import java.io.File

/** A track that exists on this phone, and which playlist put it here. */
data class LocalSong(
    val id: String,
    val title: String,
    val artist: String,
    val album: String,
    val durationSec: Int,
    val fileName: String,
    val playlist: String,
)

/**
 * The phone's record of what it has synced: one tab-separated line per track
 * in `library.tsv`.
 *
 * Not JSON, not a database. `org.json` is an Android stub that cannot be
 * unit-tested off-device, Room is a dependency and a compiler plugin for
 * something with no queries, and this file is a few hundred lines of text.
 */
object LocalLibrary {
    private const val MANIFEST = "library.tsv"

    /** Tabs and newlines are the field and record separators, so they cannot survive in a value. */
    private fun clean(value: String): String =
        value.replace('\t', ' ').replace('\n', ' ').replace('\r', ' ')

    fun encodeLine(s: LocalSong): String = listOf(
        clean(s.id),
        clean(s.title),
        clean(s.artist),
        clean(s.album),
        s.durationSec.toString(),
        clean(s.fileName),
        clean(s.playlist),
    ).joinToString("\t")

    fun decodeLine(line: String): LocalSong? {
        if (line.isBlank() || line.startsWith("#")) return null
        val f = line.split('\t')
        if (f.size != 7) return null
        return LocalSong(
            id = f[0],
            title = f[1],
            artist = f[2],
            album = f[3],
            durationSec = f[4].toIntOrNull() ?: 0,
            fileName = f[5],
            playlist = f[6],
        )
    }

    private fun manifestFile(dir: File) = File(dir, MANIFEST)

    fun tracksDir(dir: File): File = File(dir, "tracks").apply { mkdirs() }

    fun fileFor(dir: File, song: LocalSong): File = File(tracksDir(dir), song.fileName)

    fun load(dir: File): List<LocalSong> {
        val file = manifestFile(dir)
        if (!file.exists()) return emptyList()
        return file.readLines().mapNotNull { decodeLine(it) }
    }

    fun save(dir: File, songs: List<LocalSong>) {
        dir.mkdirs()
        // Write-then-rename: a killed process can't leave a truncated manifest
        // that would make synced tracks look missing.
        val tmp = File(dir, "$MANIFEST.tmp")
        tmp.writeText(songs.joinToString("\n") { encodeLine(it) })
        tmp.renameTo(manifestFile(dir))
    }
}
```

- [ ] **Step 4: Run the tests**

```sh
./gradlew :app:testDebugUnitTest
```

Expected: PASS, 12 tests total.

---

### Task 4: Pairing screen

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/PairActivity.kt` (replace the placeholder)
- Create: `android/app/src/main/java/dev/melodie/companion/Ui.kt`

**Interfaces:**
- Consumes: `parsePairingUrl`, `Pairing`, `SubsonicClient.ping()`.
- Produces: `object Ui { val BG: Int; val FG: Int; val FG_DIM: Int; val ACCENT: Int; fun column(context: Context): LinearLayout; fun label(context: Context, text: String, size: Float = 16f): TextView; fun input(context: Context, hint: String): EditText; fun button(context: Context, text: String, onClick: () -> Unit): Button }`, and a `PairActivity` that stores a `Server` and launches `LibraryActivity`.

- [ ] **Step 1: Implement `Ui.kt`**

Views are built in code, so this is the whole "theme". Colours match the desktop's `ui/theme.rs`.

```kotlin
package dev.melodie.companion

import android.content.Context
import android.graphics.Color
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

/** Views in code, not XML: no layout inflation, no AppCompat, no resources. */
object Ui {
    val BG = Color.parseColor("#1B1B1F")
    val BG_ALT = Color.parseColor("#26262B")
    val FG = Color.parseColor("#E6E6EA")
    val FG_DIM = Color.parseColor("#9A9AA4")
    val ACCENT = Color.parseColor("#E8B44A")

    fun dp(context: Context, value: Int): Int =
        (value * context.resources.displayMetrics.density).toInt()

    fun column(context: Context): LinearLayout = LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL
        setBackgroundColor(BG)
        val p = dp(context, 16)
        setPadding(p, p, p, p)
        layoutParams = ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
    }

    fun label(context: Context, text: String, size: Float = 16f): TextView =
        TextView(context).apply {
            this.text = text
            textSize = size
            setTextColor(FG)
            setPadding(0, dp(context, 4), 0, dp(context, 4))
        }

    fun dim(context: Context, text: String, size: Float = 13f): TextView =
        label(context, text, size).apply { setTextColor(FG_DIM) }

    fun input(context: Context, hint: String): EditText = EditText(context).apply {
        this.hint = hint
        setTextColor(FG)
        setHintTextColor(FG_DIM)
    }

    fun button(context: Context, text: String, onClick: () -> Unit): Button =
        Button(context).apply {
            this.text = text
            setTextColor(BG)
            setBackgroundColor(ACCENT)
            setOnClickListener { onClick() }
        }
}
```

- [ ] **Step 2: Implement `PairActivity.kt`**

```kotlin
package dev.melodie.companion

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Toast
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions

/**
 * Pairing (PLAN.md §7): scan the QR the desktop shows, or type the same
 * details by hand. No account, no login.
 */
class PairActivity : Activity() {

    private val main = Handler(Looper.getMainLooper())

    private val scanner = registerForActivityResult(ScanContract()) { result ->
        val contents = result.contents
        if (contents == null) {
            toast("Scan cancelled")
        } else {
            val server = parsePairingUrl(contents)
            if (server == null) toast("That QR code isn't a Melodie pairing code") else verify(server)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Already paired: go straight to the library.
        if (Pairing.load(this) != null) {
            startActivity(Intent(this, LibraryActivity::class.java))
            finish()
            return
        }

        val root = Ui.column(this)
        root.addView(Ui.label(this, "Pair with Melodie", 22f))
        root.addView(
            Ui.dim(
                this,
                "On the desktop, click Pair (or run `melodie --pair`) and scan the code.",
            )
        )
        root.addView(Ui.button(this, "Scan QR code") {
            scanner.launch(
                ScanOptions()
                    .setPrompt("Point at the code on your computer")
                    .setBeepEnabled(false)
                    .setOrientationLocked(false)
            )
        })

        root.addView(Ui.dim(this, "\nOr enter it manually:"))
        val host = Ui.input(this, "Host, e.g. 192.168.1.20")
        val port = Ui.input(this, "Port, e.g. 4533")
        val token = Ui.input(this, "Token")
        root.addView(host)
        root.addView(port)
        root.addView(token)
        root.addView(Ui.button(this, "Connect") {
            val portValue = port.text.toString().trim().toIntOrNull()
            if (host.text.isBlank() || portValue == null || token.text.isBlank()) {
                toast("Fill in host, port and token")
            } else {
                verify(Server(host.text.toString().trim(), portValue, token.text.toString().trim()))
            }
        })

        setContentView(root)
    }

    /** Confirms the details actually work before storing them. */
    private fun verify(server: Server) {
        toast("Connecting…")
        Thread {
            val ok = SubsonicClient(server).ping()
            main.post {
                if (!ok) {
                    toast("Couldn't reach Melodie at ${server.host}:${server.port}")
                } else {
                    Pairing.save(this, server)
                    startActivity(Intent(this, LibraryActivity::class.java))
                    finish()
                }
            }
        }.start()
    }

    private fun toast(message: String) =
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
}
```

`registerForActivityResult` needs `ComponentActivity`, which `android.app.Activity` is not. Rather than pull in `androidx.activity`, use the classic `startActivityForResult` path that ZXing also supports. Replace the `scanner` property and its use with:

```kotlin
    private val scanRequest = 49374 // IntentIntegrator.REQUEST_CODE

    // ... in the Scan button's onClick:
    com.journeyapps.barcodescanner.ScanOptions().let { options ->
        options.setPrompt("Point at the code on your computer")
        options.setBeepEnabled(false)
        options.setOrientationLocked(false)
        startActivityForResult(options.createScanIntent(this), scanRequest)
    }

    @Deprecated("classic result API — avoids depending on androidx.activity")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != scanRequest) return
        val contents = com.journeyapps.barcodescanner.ScanIntentResult
            .parseActivityResult(resultCode, data).contents
        if (contents == null) {
            toast("Scan cancelled")
            return
        }
        val server = parsePairingUrl(contents)
        if (server == null) toast("That QR code isn't a Melodie pairing code") else verify(server)
    }
```

Verify against the ZXing 4.3.0 API surface as you write it: if `ScanOptions.createScanIntent(Activity)` does not exist in this version, use `com.google.zxing.integration.android.IntentIntegrator(this).apply { setPrompt(...); setBeepEnabled(false) }.initiateScan()` and parse with `IntentIntegrator.parseActivityResult(requestCode, resultCode, data)`. Both APIs ship in 4.3.0; use whichever the artifact actually exposes.

- [ ] **Step 3: Register the scan activity in the manifest**

ZXing's `CaptureActivity` comes from the library's own manifest, so nothing is needed for it. Add `LibraryActivity` (implemented next task, but declared now so the intent resolves):

```xml
        <activity
            android:name=".LibraryActivity"
            android:exported="false" />
```

- [ ] **Step 4: Build**

```sh
cd /home/yuzy/Melodie/android && ./gradlew assembleDebug
```

Expected: `BUILD SUCCESSFUL` (after Task 5 creates `LibraryActivity`; if compiling this task alone, create a one-line placeholder `LibraryActivity` first, mirroring the placeholder pattern from Task 1).

---

### Task 5: Library screen — playlists and their tracks

**Files:**
- Create: `android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt`

**Interfaces:**
- Consumes: `Pairing.load`, `SubsonicClient.playlists()/playlist(id)`, `LocalLibrary.load`, `Ui`.
- Produces: a `LibraryActivity` with two modes (playlist list / track list), a Sync button per playlist, and a `startPlayback(songs, index)` hook that Task 7 fills in.

- [ ] **Step 1: Implement `LibraryActivity.kt`**

```kotlin
package dev.melodie.companion

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.ArrayAdapter
import android.widget.LinearLayout
import android.widget.ListView
import android.widget.Toast

/**
 * One screen, two modes: the server's playlists, and the tracks inside the
 * one you tapped. Playlists are synced individually — mirroring the whole
 * library onto a phone is rarely what anyone wants.
 */
class LibraryActivity : Activity() {

    private val main = Handler(Looper.getMainLooper())
    private lateinit var server: Server
    private lateinit var client: SubsonicClient
    private lateinit var root: LinearLayout
    private lateinit var status: android.widget.TextView
    private lateinit var list: ListView

    private var playlists: List<RemotePlaylist> = emptyList()
    private var openPlaylist: RemotePlaylist? = null
    private var songs: List<RemoteSong> = emptyList()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val paired = Pairing.load(this)
        if (paired == null) {
            startActivity(Intent(this, PairActivity::class.java))
            finish()
            return
        }
        server = paired
        client = SubsonicClient(server)

        root = Ui.column(this)
        status = Ui.dim(this, "Loading…")
        list = ListView(this).apply {
            setBackgroundColor(Ui.BG)
            divider = null
        }
        root.addView(Ui.label(this, "Melodie", 22f))
        root.addView(status)
        root.addView(list)
        setContentView(root)

        // The media notification needs this on Android 13+; without it
        // playback still works but shows no controls.
        if (Build.VERSION.SDK_INT >= 33) {
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        }

        loadPlaylists()
    }

    override fun onBackPressed() {
        if (openPlaylist != null) showPlaylists() else super.onBackPressed()
    }

    private fun loadPlaylists() {
        status.text = "Loading playlists…"
        Thread {
            val result = runCatching { client.playlists() }
            main.post {
                result.onSuccess {
                    playlists = it
                    showPlaylists()
                }.onFailure { e ->
                    status.text = "Can't reach Melodie: ${e.message}"
                }
            }
        }.start()
    }

    private fun showPlaylists() {
        openPlaylist = null
        val local = LocalLibrary.load(filesDir).groupBy { it.playlist }
        val rows = playlists.map { p ->
            val synced = local[p.name]?.size ?: 0
            if (synced > 0) "${p.name}\n$synced of ${p.songCount} on this phone"
            else "${p.name}\n${p.songCount} tracks — not synced"
        }
        status.text = if (playlists.isEmpty()) "No playlists on the server yet." else "Tap a playlist."
        list.adapter = darkAdapter(rows)
        list.setOnItemClickListener { _, _, index, _ -> openPlaylist(playlists[index]) }
    }

    private fun openPlaylist(playlist: RemotePlaylist) {
        openPlaylist = playlist
        status.text = "Loading ${playlist.name}…"
        Thread {
            val result = runCatching { client.playlist(playlist.id) }
            main.post {
                result.onSuccess {
                    songs = it
                    showSongs(playlist)
                }.onFailure { e ->
                    status.text = "Couldn't load ${playlist.name}: ${e.message}"
                }
            }
        }.start()
    }

    private fun showSongs(playlist: RemotePlaylist) {
        val local = LocalLibrary.load(filesDir).filter { it.playlist == playlist.name }
        val haveIds = local.map { it.id }.toSet()
        val rows = songs.map { s ->
            val mark = if (s.id in haveIds) "" else "  (not synced)"
            "${s.title}\n${s.artist}$mark"
        }
        status.text = "${playlist.name} — ${haveIds.size}/${songs.size} on this phone"
        list.adapter = darkAdapter(rows)
        list.setOnItemClickListener { _, _, index, _ ->
            val song = songs[index]
            val localSong = local.firstOrNull { it.id == song.id }
            if (localSong == null) {
                Toast.makeText(this, "Sync this playlist first", Toast.LENGTH_SHORT).show()
            } else {
                val ordered = songs.mapNotNull { s -> local.firstOrNull { it.id == s.id } }
                val start = ordered.indexOfFirst { it.id == song.id }
                startPlayback(ordered, if (start < 0) 0 else start)
            }
        }
        syncButton(playlist)
    }

    private var syncButtonView: android.widget.Button? = null

    private fun syncButton(playlist: RemotePlaylist) {
        syncButtonView?.let { root.removeView(it) }
        val button = Ui.button(this, "Sync “${playlist.name}” to this phone") {
            startSync(playlist)
        }
        syncButtonView = button
        root.addView(button)
    }

    /** Filled in by Task 6. */
    private fun startSync(playlist: RemotePlaylist) {
        Toast.makeText(this, "Sync not wired up yet", Toast.LENGTH_SHORT).show()
    }

    /** Filled in by Task 7. */
    private fun startPlayback(queue: List<LocalSong>, index: Int) {
        Toast.makeText(this, "Playback not wired up yet", Toast.LENGTH_SHORT).show()
    }

    private fun darkAdapter(rows: List<String>): ArrayAdapter<String> =
        object : ArrayAdapter<String>(this, android.R.layout.simple_list_item_1, rows) {
            override fun getView(position: Int, convertView: android.view.View?, parent: android.view.ViewGroup): android.view.View {
                val view = super.getView(position, convertView, parent)
                (view as android.widget.TextView).setTextColor(Ui.FG)
                return view
            }
        }
}
```

- [ ] **Step 2: Build**

```sh
cd /home/yuzy/Melodie/android && ./gradlew assembleDebug
```

Expected: `BUILD SUCCESSFUL`.

- [ ] **Step 3: Re-run the unit tests to be sure nothing regressed**

```sh
./gradlew :app:testDebugUnitTest
```

Expected: PASS, 12 tests.

---

### Task 6: Sync — download a playlist to the phone

**Files:**
- Create: `android/app/src/main/java/dev/melodie/companion/Sync.kt`
- Modify: `android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt` (replace `startSync`)

**Interfaces:**
- Consumes: `SubsonicClient.downloadTo`, `LocalLibrary`.
- Produces: `object Sync { fun playlist(filesDir: File, client: SubsonicClient, playlistName: String, songs: List<RemoteSong>, onProgress: (done: Int, total: Int, title: String) -> Unit, onDone: (synced: Int, failed: Int) -> Unit) }`

- [ ] **Step 1: Implement `Sync.kt`**

```kotlin
package dev.melodie.companion

import java.io.File

/**
 * Downloads a playlist's tracks into app-private storage, one at a time.
 *
 * Serial on purpose, mirroring the desktop's own fetch policy (PLAN.md
 * §5.5): a personal library is not a race, and one socket at a time keeps
 * both memory and the phone's radio behaviour predictable.
 */
object Sync {

    fun playlist(
        filesDir: File,
        client: SubsonicClient,
        playlistName: String,
        songs: List<RemoteSong>,
        onProgress: (done: Int, total: Int, title: String) -> Unit,
        onDone: (synced: Int, failed: Int) -> Unit,
    ) {
        Thread {
            val existing = LocalLibrary.load(filesDir).toMutableList()
            val tracks = LocalLibrary.tracksDir(filesDir)
            var failed = 0

            songs.forEachIndexed { index, song ->
                onProgress(index, songs.size, song.title)
                val fileName = "${song.id}.${song.suffix}"
                val target = File(tracks, fileName)
                val entry = LocalSong(
                    id = song.id,
                    title = song.title,
                    artist = song.artist,
                    album = song.album,
                    durationSec = song.durationSec,
                    fileName = fileName,
                    playlist = playlistName,
                )

                val alreadyHere = target.exists() && target.length() > 0
                if (!alreadyHere) {
                    val result = runCatching { client.downloadTo(song.id, target) { } }
                    if (result.isFailure) {
                        failed++
                        return@forEachIndexed
                    }
                }
                // Re-syncing a playlist must not duplicate rows.
                existing.removeAll { it.id == song.id && it.playlist == playlistName }
                existing.add(entry)
            }

            // Tracks dropped from the playlist upstream lose their manifest
            // row; the file itself is left alone. Deleting a user's audio
            // because a remote list changed is the wrong default (PLAN.md
            // §5.3 makes the same call on the desktop).
            val keep = songs.map { it.id }.toSet()
            existing.removeAll { it.playlist == playlistName && it.id !in keep }

            LocalLibrary.save(filesDir, existing)
            onDone(songs.size - failed, failed)
        }.start()
    }
}
```

- [ ] **Step 2: Wire it into `LibraryActivity`**

Replace the placeholder `startSync` with:

```kotlin
    private fun startSync(playlist: RemotePlaylist) {
        val queued = songs
        if (queued.isEmpty()) return
        status.text = "Syncing ${playlist.name}…"
        Sync.playlist(
            filesDir = filesDir,
            client = client,
            playlistName = playlist.name,
            songs = queued,
            onProgress = { done, total, title ->
                main.post { status.text = "Syncing ${done + 1}/$total — $title" }
            },
            onDone = { synced, failed ->
                main.post {
                    status.text = if (failed == 0) {
                        "${playlist.name}: $synced tracks on this phone"
                    } else {
                        "${playlist.name}: $synced synced, $failed failed"
                    }
                    showSongs(playlist)
                }
            },
        )
    }
```

- [ ] **Step 3: Build and test**

```sh
cd /home/yuzy/Melodie/android && ./gradlew assembleDebug :app:testDebugUnitTest
```

Expected: `BUILD SUCCESSFUL`, 12 tests pass.

---

### Task 7: Background playback — MediaSessionService and the player screen

**Files:**
- Create: `android/app/src/main/java/dev/melodie/companion/PlaybackService.kt`
- Create: `android/app/src/main/java/dev/melodie/companion/PlayerActivity.kt`
- Modify: `android/app/src/main/AndroidManifest.xml`
- Modify: `android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt` (replace `startPlayback`)

**Interfaces:**
- Produces:
  - `class PlaybackService : MediaSessionService`
  - `class PlayerActivity : Activity` with `companion object { const val EXTRA_INDEX = "index" }`
  - `object Queue { var pending: List<LocalSong> }` — the handoff from library to player.

- [ ] **Step 1: Implement `PlaybackService.kt`**

```kotlin
package dev.melodie.companion

import android.app.PendingIntent
import android.content.Intent
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService

/**
 * PLAN.md §1/§7: reliable background audio on Android means a
 * MediaSessionService holding the player. This is what keeps playback alive
 * with the app off-screen and gives lockscreen, headset and Bluetooth
 * controls for free.
 */
class PlaybackService : MediaSessionService() {

    private var session: MediaSession? = null

    override fun onCreate() {
        super.onCreate()
        val player = ExoPlayer.Builder(this).build()
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, PlayerActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        session = MediaSession.Builder(this, player)
            .setSessionActivity(open)
            .build()
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Swiping the app away while paused should not leave a silent
        // service running; while playing, playback continues on purpose.
        val player = session?.player
        if (player == null || !player.isPlaying) {
            stopSelf()
        }
    }

    override fun onDestroy() {
        session?.run {
            player.release()
            release()
        }
        session = null
        super.onDestroy()
    }
}
```

- [ ] **Step 2: Declare the service**

Add inside `<application>` in the manifest:

```xml
        <service
            android:name=".PlaybackService"
            android:exported="true"
            android:foregroundServiceType="mediaPlayback">
            <intent-filter>
                <action android:name="androidx.media3.session.MediaSessionService" />
            </intent-filter>
        </service>

        <activity
            android:name=".PlayerActivity"
            android:exported="false" />
```

- [ ] **Step 3: Implement `PlayerActivity.kt`**

```kotlin
package dev.melodie.companion

import android.app.Activity
import android.content.ComponentName
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.TextView
import androidx.core.content.ContextCompat
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import java.io.File

/** Handoff from the library screen; cleared as soon as it is consumed. */
object Queue {
    var pending: List<LocalSong> = emptyList()
}

class PlayerActivity : Activity() {

    companion object {
        const val EXTRA_INDEX = "index"
    }

    private val main = Handler(Looper.getMainLooper())
    private var controller: MediaController? = null
    private lateinit var nowPlaying: TextView
    private lateinit var position: TextView
    private lateinit var seek: SeekBar
    private lateinit var playPause: android.widget.Button
    private var userIsSeeking = false

    private val tick = object : Runnable {
        override fun run() {
            val c = controller ?: return
            if (!userIsSeeking && c.duration > 0) {
                seek.max = (c.duration / 1000).toInt()
                seek.progress = (c.currentPosition / 1000).toInt()
                position.text = "${clock(c.currentPosition)} / ${clock(c.duration)}"
            }
            playPause.text = if (c.isPlaying) "Pause" else "Play"
            nowPlaying.text = c.mediaMetadata.title?.toString() ?: "Nothing playing"
            // Only ticks while this screen is visible — stopped in onStop.
            main.postDelayed(this, 1000)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root: LinearLayout = Ui.column(this)
        nowPlaying = Ui.label(this, "Connecting…", 20f)
        position = Ui.dim(this, "0:00 / 0:00")
        seek = SeekBar(this)
        playPause = Ui.button(this, "Play") { toggle() }
        root.addView(nowPlaying)
        root.addView(position)
        root.addView(seek)
        root.addView(playPause)
        root.addView(Ui.button(this, "Previous") { controller?.seekToPreviousMediaItem() })
        root.addView(Ui.button(this, "Next") { controller?.seekToNextMediaItem() })
        setContentView(root)

        seek.setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(bar: SeekBar?, value: Int, fromUser: Boolean) {}
            override fun onStartTrackingTouch(bar: SeekBar?) { userIsSeeking = true }
            override fun onStopTrackingTouch(bar: SeekBar?) {
                userIsSeeking = false
                controller?.seekTo(seek.progress * 1000L)
            }
        })

        connect(intent.getIntExtra(EXTRA_INDEX, 0))
    }

    private fun connect(startIndex: Int) {
        val token = SessionToken(this, ComponentName(this, PlaybackService::class.java))
        val future = MediaController.Builder(this, token).buildAsync()
        future.addListener({
            val c = future.get()
            controller = c
            val queue = Queue.pending
            if (queue.isNotEmpty()) {
                Queue.pending = emptyList()
                c.setMediaItems(queue.map { toMediaItem(it) }, startIndex, 0L)
                c.prepare()
                c.play()
            }
            main.post(tick)
        }, ContextCompat.getMainExecutor(this))
    }

    private fun toMediaItem(song: LocalSong): MediaItem = MediaItem.Builder()
        .setUri(Uri.fromFile(File(LocalLibrary.tracksDir(filesDir), song.fileName)))
        .setMediaId(song.id)
        .setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(song.title)
                .setArtist(song.artist)
                .setAlbumTitle(song.album)
                .build()
        )
        .build()

    private fun toggle() {
        val c = controller ?: return
        if (c.isPlaying) c.pause() else c.play()
    }

    private fun clock(ms: Long): String {
        val total = (ms / 1000).coerceAtLeast(0)
        return "${total / 60}:${(total % 60).toString().padStart(2, '0')}"
    }

    override fun onStop() {
        super.onStop()
        main.removeCallbacks(tick)
    }

    override fun onDestroy() {
        main.removeCallbacks(tick)
        controller?.release()
        controller = null
        super.onDestroy()
    }
}
```

- [ ] **Step 4: Wire playback into `LibraryActivity`**

Replace the placeholder `startPlayback` with:

```kotlin
    private fun startPlayback(queue: List<LocalSong>, index: Int) {
        Queue.pending = queue
        startActivity(
            Intent(this, PlayerActivity::class.java)
                .putExtra(PlayerActivity.EXTRA_INDEX, index)
        )
    }
```

- [ ] **Step 5: Build and test**

```sh
cd /home/yuzy/Melodie/android && ./gradlew assembleDebug :app:testDebugUnitTest
```

Expected: `BUILD SUCCESSFUL`, 12 tests pass.

- [ ] **Step 6: Verify the service and activities landed in the APK**

```sh
$HOME/Android/Sdk/build-tools/36.0.0/aapt2 dump xmltree \
  --file AndroidManifest.xml app/build/outputs/apk/debug/app-debug.apk | grep -E "activity|service" -A2
```

Expected: `PairActivity`, `LibraryActivity`, `PlayerActivity` and `PlaybackService` are all present, and the service carries `foregroundServiceType=mediaPlayback`.

---

### Task 8: Shrink, package, and hand it over

**Files:**
- Modify: `README.md`
- Create: `docs/android.md`

- [ ] **Step 1: Build the shrunk release APK**

```sh
cd /home/yuzy/Melodie/android
./gradlew assembleRelease
ls -lh app/build/outputs/apk/release/app-release.apk
```

Expected: `BUILD SUCCESSFUL` and an APK. If R8 fails on a missing class, add the specific `-dontwarn`/`-keep` line it names to `app/proguard-rules.pro` — do not disable minification wholesale.

- [ ] **Step 2: Verify the release APK is signed and well-formed**

```sh
$HOME/Android/Sdk/build-tools/36.0.0/apksigner verify --print-certs \
  app/build/outputs/apk/release/app-release.apk
$HOME/Android/Sdk/build-tools/36.0.0/aapt2 dump badging \
  app/build/outputs/apk/release/app-release.apk | head -3
```

Expected: `apksigner` reports a valid v2/v3 signature with CN=Android Debug; badging shows `dev.melodie.companion`.

- [ ] **Step 3: Confirm R8 kept the entry points**

```sh
unzip -l app/build/outputs/apk/release/app-release.apk | grep -E "classes.*dex|resources.arsc"
$HOME/Android/Sdk/cmdline-tools/latest/bin/apkanalyzer dex packages \
  app/build/outputs/apk/release/app-release.apk 2>/dev/null | grep -E "dev.melodie.companion" | head
```

Expected: `dev.melodie.companion`'s classes are present in the dex. (If `apkanalyzer` is not installed, `unzip -p ... classes.dex | strings | grep melodie` is enough to confirm.)

- [ ] **Step 4: Record the measured size**

```sh
du -h app/build/outputs/apk/release/app-release.apk
du -h app/build/outputs/apk/debug/app-debug.apk
```

Write both numbers down. PLAN.md §7 estimated 2-4 MB; report the real figure and say plainly whether it met that, rather than restating the estimate.

- [ ] **Step 5: Copy the APK somewhere obvious**

```sh
cp app/build/outputs/apk/release/app-release.apk /home/yuzy/Melodie/melodie-companion.apk
ls -lh /home/yuzy/Melodie/melodie-companion.apk
```

- [ ] **Step 6: Write `docs/android.md`**

```markdown
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
- Tap a synced track to play. Playback continues with the screen off and the
  app closed, with lockscreen and Bluetooth controls.
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
```

- [ ] **Step 7: Update `README.md`**

1. Change "This build implements **Phases 0–3**" to Phases 0–5, and reduce the not-implemented list to Phases 6–7.
2. Add a **Phase 5** bullet under "What's implemented": Kotlin/Views companion app, QR pairing, playlist-selective offline sync, Media3 `MediaSessionService` background playback; measured APK size.
3. Add a short "Android companion" section pointing at `docs/android.md` and naming the APK path.
4. Add to "Deviations from PLAN.md":
   - *Playlist-selective sync rather than whole-library mirroring* — PLAN.md §7 says "syncs files over the Tier 0 endpoint into app-private storage"; syncing everything would fill a phone, so sync is per-playlist and opt-in.
   - *No background/scheduled sync* — sync runs on demand while the app is open; WorkManager was not worth a dependency for a manual mirror.
   - *The APK is signed with the debug key*, deliberately, so there is no keystore to manage for a personal sideload.
   - *Measured APK size vs PLAN.md §7's 2-4 MB estimate* — state the real number.

- [ ] **Step 8: Final verification**

```sh
cd /home/yuzy/Melodie/android
./gradlew clean assembleRelease :app:testDebugUnitTest
ls -lh app/build/outputs/apk/release/app-release.apk
cd /home/yuzy/Melodie && cargo test
```

Expected: a clean build from scratch, unit tests pass, Rust tests still pass.

---

## Self-Review

**Spec coverage:**

| Spec item | Task |
|---|---|
| Gradle project, Kotlin, Views, minSdk 24 | 1 |
| media3-exoplayer + media3-session deps | 1 |
| zxing-android-embedded (justified over ML Kit) | 1 |
| No Room/OkHttp/WorkManager/coroutines | enforced throughout; Global Constraints |
| `HttpURLConnection` for Subsonic calls | 2 |
| `XmlPullParser` for responses | 2 |
| Pairing screen: QR + manual fallback | 4 |
| Playlists → tracks list screen | 5 |
| Playlist-selective sync into app-private storage | 6 |
| Local manifest of synced tracks | 3 (TSV, replacing the spec's `library.json` — same role, and unlike `org.json` it is unit-testable off-device; noted as a deviation in Task 8) |
| `MediaSessionService` + ExoPlayer player screen | 7 |
| SharedPreferences pairing storage | 2 (`Pairing`) |
| SDK bootstrap under `~/Android/Sdk` | 1 |
| Debug-signed APK | 1 (debug), 8 (release signed with debug key) |
| minify/shrinkResources | 1 (config), 8 (build + verify) |
| Report measured APK size | 8 |
| Sideload instructions | 8 |
| No emulator — structural verification only | 8 |

One spec deviation, deliberate and recorded: the manifest is `library.tsv`, not `library.json`. Reason: `org.json` is an Android stub that throws in JVM unit tests, so a JSON manifest could not carry a test; the file's role, location and contents are otherwise identical.

**Placeholder scan:** `startSync` and `startPlayback` are placeholders *by design* in Tasks 5, with their real bodies given in full in Tasks 6 and 7 — each is a named step, not a "TBD". No other placeholders.

**Type consistency:** `Server(host, port, token)` is constructed identically in Tasks 2, 4, 5. `RemoteSong.suffix` feeds the filename in Task 6 and `LocalSong.fileName` is consumed by `LocalLibrary.fileFor`/`PlayerActivity.toMediaItem` in Task 7. `LocalSong`'s 7 fields match the 7 TSV columns in Task 3's `encodeLine`/`decodeLine`. `Sync.playlist`'s `onProgress(done, total, title)` and `onDone(synced, failed)` match the call site in Task 6. `PlayerActivity.EXTRA_INDEX` is defined in Task 7 and used by `LibraryActivity` in the same task.

**Known risk, stated rather than hidden:** ZXing 4.3.0's scan-launch API is used in Task 4 via the classic `startActivityForResult` path to avoid an `androidx.activity` dependency; the exact entry point (`ScanOptions.createScanIntent` vs `IntentIntegrator.initiateScan`) must be confirmed against the artifact while implementing, and Task 4 says so explicitly.

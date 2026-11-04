import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// A keystore named in the environment, as CI's secrets give it, signs
// release builds, so that each one installs over the last and Google Play
// takes them; without one they are signed with this machine's debug key.
// Its key is the one aliased qdp, and has the keystore's password, if it has
// one, as PKCS12 keystores' keys do, unless the environment names others.
val releaseKeystore = System.getenv("QDP_KEYSTORE")?.takeIf { it.isNotEmpty() }?.let(::file)
val keystorePassword = System.getenv("QDP_KEYSTORE_PASSWORD").orEmpty()

// The workspace's version, from the root Cargo.toml.
val workspaceVersion = rootProject.file("../Cargo.toml").readLines()
    .first { it.startsWith("version = \"") }
    .substringAfter('"')
    .substringBefore('"')

android {
    namespace = "io.github.ianmarmour.qdp"
    compileSdk = 36

    defaultConfig {
        applicationId = "io.github.ianmarmour.qdp"
        // Android 10: MediaCodec decodes AV1 on devices with an AV1
        // decoder, and AAudio plays with low latency.
        minSdk = 29
        // Google Play takes new apps and updates only if they target
        // Android 16.
        targetSdk = 36
        // CI numbers its builds, so that each installs over the one before.
        versionCode = System.getenv("QDP_VERSION_CODE")?.toIntOrNull() ?: 1
        versionName = System.getenv("QDP_VERSION_NAME")?.takeIf { it.isNotEmpty() } ?: workspaceVersion
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = releaseKeystore
                storePassword = keystorePassword
                keyAlias = System.getenv("QDP_KEY_ALIAS")?.takeIf { it.isNotEmpty() } ?: "qdp"
                keyPassword = System.getenv("QDP_KEY_PASSWORD")?.takeIf { it.isNotEmpty() } ?: keystorePassword
            }
        }
    }

    buildTypes {
        release {
            // R8 shrinks Compose from about 22 MB of dex to a few hundred
            // kilobytes; proguard-rules.pro keeps the Rust core's JNI entry
            // points.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.getByName(if (releaseKeystore != null) "release" else "debug")
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

// These are the newest releases that build with AGP 8 (Compose UI 1.11 and
// Material 3 1.4); newer ones need AGP 9, and some compileSdk 37, a
// toolchain change of its own, so lint's reminders of them are silenced
// here.
dependencies {
    //noinspection GradleDependency
    implementation(platform("androidx.compose:compose-bom:2026.06.01"))
    implementation("androidx.compose.material3:material3")
    //noinspection GradleDependency
    implementation("androidx.activity:activity-compose:1.10.1")
    //noinspection GradleDependency
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    //noinspection GradleDependency
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    //noinspection GradleDependency
    implementation("androidx.compose.material3.adaptive:adaptive:1.2.0")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
}

// Builds the Rust core into src/main/jniLibs before each build; pass
// -PskipCore to package the library already there.
val buildCore by tasks.registering(Exec::class) {
    description = "Builds libqdp_android.so with cargo"
    workingDir = rootDir
    commandLine(rootDir.resolve("build-core.sh").absolutePath)
}

if (!project.hasProperty("skipCore")) {
    tasks.named("preBuild") {
        dependsOn(buildCore)
    }
}

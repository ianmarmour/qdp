import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// A keystore named in the environment, as CI's secrets give it, signs
// release builds, so that each one installs over the last; without one they
// are signed with this machine's debug key.
val releaseKeystore = System.getenv("QDP_KEYSTORE")?.takeIf { it.isNotEmpty() }?.let(::file)

android {
    namespace = "io.github.ianmarmour.qdp"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.ianmarmour.qdp"
        // Android 10: MediaCodec decodes AV1 on devices with an AV1
        // decoder, and AAudio plays with low latency.
        minSdk = 29
        targetSdk = 35
        // CI numbers its builds, so that each installs over the one before.
        versionCode = System.getenv("QDP_VERSION_CODE")?.toIntOrNull() ?: 1
        versionName = System.getenv("QDP_VERSION_NAME")?.takeIf { it.isNotEmpty() } ?: "0.1.0"
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = releaseKeystore
                storePassword = System.getenv("QDP_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("QDP_KEY_ALIAS")
                keyPassword = System.getenv("QDP_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName(if (releaseKeystore != null) "release" else "debug")
        }
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

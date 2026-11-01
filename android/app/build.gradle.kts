import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "io.github.ianmarmour.qdp"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.ianmarmour.qdp"
        // Android 10: MediaCodec decodes AV1 on devices with an AV1
        // decoder, and AAudio plays with low latency.
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Signed with the debug key so that a release build installs
            // without a keystore of its own; sign it properly to publish.
            signingConfig = signingConfigs.getByName("debug")
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

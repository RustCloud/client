// Built inside the app's generated Gradle project (see release.yml), which puts
// the Android and Kotlin plugins on the classpath.
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.rustcloud.probe"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.rustcloud.probe"
        minSdk = 24
        targetSdk = 36
        versionCode = 1
        versionName = "1.0"
    }

    buildTypes {
        // Signed in CI with the app's key: one of our apps, let in silently.
        release {
            isMinifyEnabled = false
        }
        // Signed with Gradle's debug key: another developer's app, which the user
        // is asked about. Its own id, so both install side by side.
        debug {
            applicationIdSuffix = ".other"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}
